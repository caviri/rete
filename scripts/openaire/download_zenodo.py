"""Download every file of a Zenodo record — resumable, MD5-verified.

Default target is the OpenAIRE Graph Dataset v11.1.1
(https://zenodo.org/records/20428976: 67 tar files, 352.4 GiB, CC-BY-4.0)
into data/openaire/2026.

- Each file downloads to `<name>.part` and is renamed only after its MD5
  matches the checksum published in the record API, so a finished filename
  is always a verified file. Interrupted runs resume from the .part byte
  offset via HTTP Range.
- Verified files are remembered in `_verified.json` (keyed by name+md5) so
  re-runs don't re-hash hundreds of GiB.
- The record metadata is saved alongside the data as `record.json`.
- Before each file the free disk space is checked; the run aborts cleanly
  (resumable) rather than filling the disk.

Usage:
  python scripts/openaire/download_zenodo.py                  # everything
  python scripts/openaire/download_zenodo.py --list           # show files only
  python scripts/openaire/download_zenodo.py --only 'publication_*,software.tar'
  python scripts/openaire/download_zenodo.py --exclude 'product_Cites_*'
  python scripts/openaire/download_zenodo.py --record 20428976 --out data/openaire/2026
"""

import argparse
import fnmatch
import hashlib
import json
import shutil
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_RECORD = "20428976"
DEFAULT_OUT = Path(__file__).resolve().parents[2] / "data" / "openaire" / "2026"
USER_AGENT = "rete-zenodo-fetch/1.0 (https://github.com/caviri/rete)"
CHUNK = 1 << 20  # 1 MiB read chunks
MAX_ATTEMPTS = 8
GiB = 1 << 30


def http_get(url, byte_start=0, timeout=60):
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    if byte_start > 0:
        req.add_header("Range", f"bytes={byte_start}-")
    return urllib.request.urlopen(req, timeout=timeout)


def fetch_record(record_id):
    url = f"https://zenodo.org/api/records/{record_id}"
    for attempt in range(1, MAX_ATTEMPTS + 1):
        try:
            with http_get(url) as resp:
                return json.load(resp)
        except urllib.error.HTTPError as e:
            if e.code == 429:
                wait = int(e.headers.get("Retry-After", 60))
                print(f"  rate-limited fetching record, waiting {wait}s")
                time.sleep(wait)
                continue
            raise
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            if attempt == MAX_ATTEMPTS:
                raise
            print(f"  record fetch failed ({e}), retry {attempt}/{MAX_ATTEMPTS}")
            time.sleep(min(2**attempt, 60))
    raise RuntimeError("unreachable")


def md5_of(path):
    h = hashlib.md5()
    with open(path, "rb") as f:
        while chunk := f.read(CHUNK * 8):
            h.update(chunk)
    return h.hexdigest()


def fmt_bytes(n):
    return f"{n / GiB:.2f} GiB" if n >= GiB else f"{n / (1 << 20):.1f} MiB"


def download_one(entry, out_dir, verified, verified_path):
    name, size = entry["key"], entry["size"]
    md5_expected = entry["checksum"].split(":", 1)[1]
    url = entry["links"]["self"]
    final = out_dir / name
    part = out_dir / (name + ".part")

    if verified.get(name) == md5_expected and final.exists() and final.stat().st_size == size:
        print(f"  {name}: already verified, skipping")
        return True
    if final.exists() and final.stat().st_size == size:
        # present from an earlier run but not in the state file: hash it once
        print(f"  {name}: present, verifying MD5…", flush=True)
        if md5_of(final) == md5_expected:
            verified[name] = md5_expected
            verified_path.write_text(json.dumps(verified, indent=1))
            print(f"  {name}: OK")
            return True
        print(f"  {name}: MD5 MISMATCH on existing file — re-downloading")
        final.rename(part)

    free = shutil.disk_usage(out_dir).free
    have = part.stat().st_size if part.exists() else 0
    needed = size - have + 10 * GiB  # keep a 10 GiB safety margin
    if free < needed:
        print(f"  {name}: NOT ENOUGH DISK — need {fmt_bytes(needed)} free "
              f"(incl. 10 GiB margin), have {fmt_bytes(free)}. Aborting; "
              f"re-run after freeing space to resume.")
        return False

    for attempt in range(1, MAX_ATTEMPTS + 1):
        have = part.stat().st_size if part.exists() else 0
        if have > size:
            print(f"  {name}: partial larger than expected, restarting")
            part.unlink()
            have = 0
        try:
            with http_get(url, byte_start=have) as resp:
                if have > 0 and resp.status != 206:
                    # server ignored the Range header; start over
                    have = 0
                mode = "ab" if have > 0 else "wb"
                done = have
                t0 = t_report = time.monotonic()
                rep_bytes = done
                with open(part, mode) as f:
                    while chunk := resp.read(CHUNK):
                        f.write(chunk)
                        done += len(chunk)
                        now = time.monotonic()
                        if now - t_report >= 5:
                            speed = (done - rep_bytes) / (now - t_report)
                            eta = (size - done) / speed if speed else 0
                            print(f"  {name}: {100 * done / size:5.1f}%  "
                                  f"{fmt_bytes(done)}/{fmt_bytes(size)}  "
                                  f"{speed / (1 << 20):6.1f} MiB/s  "
                                  f"ETA {eta / 60:5.1f} min", flush=True)
                            t_report, rep_bytes = now, done
            if part.stat().st_size != size:
                raise OSError(f"connection ended early at {part.stat().st_size}/{size}")
            print(f"  {name}: downloaded in {(time.monotonic() - t0) / 60:.1f} min, verifying MD5…",
                  flush=True)
            if md5_of(part) != md5_expected:
                print(f"  {name}: MD5 MISMATCH after download, retrying from scratch")
                part.unlink()
                continue
            part.rename(final)
            verified[name] = md5_expected
            verified_path.write_text(json.dumps(verified, indent=1))
            print(f"  {name}: OK")
            return True
        except urllib.error.HTTPError as e:
            if e.code == 429:
                wait = int(e.headers.get("Retry-After", 60))
                print(f"  {name}: rate-limited, waiting {wait}s")
                time.sleep(wait)
            elif e.code in (500, 502, 503, 504):
                print(f"  {name}: HTTP {e.code}, retry {attempt}/{MAX_ATTEMPTS}")
                time.sleep(min(2**attempt, 120))
            else:
                print(f"  {name}: HTTP {e.code} — giving up on this file")
                return False
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            print(f"  {name}: {e}, retry {attempt}/{MAX_ATTEMPTS} (resumes at byte offset)")
            time.sleep(min(2**attempt, 120))
    print(f"  {name}: FAILED after {MAX_ATTEMPTS} attempts")
    return False


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--record", default=DEFAULT_RECORD, help="Zenodo record id")
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT, help="output directory")
    ap.add_argument("--only", default="", help="comma-separated glob(s) of files to include")
    ap.add_argument("--exclude", default="", help="comma-separated glob(s) of files to skip")
    ap.add_argument("--list", action="store_true", help="list the record's files and exit")
    args = ap.parse_args()

    record = fetch_record(args.record)
    files = sorted(record.get("files", []), key=lambda fe: fe["key"])
    if args.only:
        pats = [p.strip() for p in args.only.split(",") if p.strip()]
        files = [fe for fe in files if any(fnmatch.fnmatch(fe["key"], p) for p in pats)]
    if args.exclude:
        pats = [p.strip() for p in args.exclude.split(",") if p.strip()]
        files = [fe for fe in files if not any(fnmatch.fnmatch(fe["key"], p) for p in pats)]
    total = sum(fe["size"] for fe in files)

    title = record.get("metadata", {}).get("title", "?")
    version = record.get("metadata", {}).get("version", "?")
    print(f"record {args.record}: {title} v{version}")
    print(f"{len(files)} files selected, {fmt_bytes(total)} total")
    if args.list:
        for fe in files:
            print(f"  {fe['key']:60s} {fmt_bytes(fe['size']):>12s}")
        return 0
    if not files:
        print("nothing matched")
        return 1

    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "record.json").write_text(json.dumps(record, indent=1), encoding="utf-8")

    verified_path = args.out / "_verified.json"
    verified = json.loads(verified_path.read_text()) if verified_path.exists() else {}
    remaining = sum(
        fe["size"] for fe in files
        if verified.get(fe["key"]) != fe["checksum"].split(":", 1)[1]
    )
    free = shutil.disk_usage(args.out).free
    print(f"still to download: {fmt_bytes(remaining)}; free disk: {fmt_bytes(free)}")
    if free < remaining + 10 * GiB:
        print(f"WARNING: not enough free space for the full selection "
              f"(need ~{fmt_bytes(remaining + 10 * GiB)}). The run will stop "
              f"cleanly when the disk margin is hit; free space and re-run to resume.")

    ok = fail = 0
    for i, fe in enumerate(files, 1):
        print(f"[{i}/{len(files)}] {fe['key']} ({fmt_bytes(fe['size'])})")
        if download_one(fe, args.out, verified, verified_path):
            ok += 1
        else:
            fail += 1
            free = shutil.disk_usage(args.out).free
            if free < 11 * GiB:
                print("stopping: disk nearly full")
                break
    print(f"done: {ok} verified, {fail} failed/skipped")
    return 0 if fail == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
