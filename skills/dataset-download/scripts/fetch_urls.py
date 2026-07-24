#!/usr/bin/env python3
"""Parallel, resumable downloader for a list of URLs. Stdlib only, so it runs in
a plain `python:3.12-slim` container with no pip install.

    python fetch_urls.py <manifest> <dest_dir> [--workers N] [--s3-host HOST]

<manifest> is a text file with one URL per line, OR a TSV whose LAST
whitespace/tab-separated field on each line is the URL (a leading `#`/blank line
or a header line without an http URL is skipped). Files are named by the URL's
last path segment and written into <dest_dir>.

Behaviour:
  - skips files already present and non-empty (resume-safe: just re-run)
  - downloads to <file>.part then atomically renames (never leaves half-files)
  - retries each URL with a few attempts
  - maps s3://<bucket>/<key> -> https://<bucket>.<--s3-host>/<key> if --s3-host given
  - writes misses to <dest_dir>/download_failures.txt (re-run retries only those)

Tip: use a lower --workers (~8) for large multi-MB files — high concurrency of
big writes is what tips a Docker bind-mount into I/O errors.
"""
from __future__ import annotations

import sys
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path


def parse_args(argv: list[str]):
    workers = 16
    s3_host = ""
    pos: list[str] = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--workers":
            workers = int(argv[i + 1]); i += 2; continue
        if a == "--s3-host":
            s3_host = argv[i + 1].strip("/"); i += 2; continue
        pos.append(a); i += 1
    if len(pos) < 2:
        sys.exit("usage: fetch_urls.py <manifest> <dest_dir> [--workers N] [--s3-host HOST]")
    return Path(pos[0]), Path(pos[1]), workers, s3_host


def normalise(url: str, s3_host: str) -> str:
    if s3_host and url.startswith("s3://"):
        rest = url[len("s3://"):]
        bucket, _, key = rest.partition("/")
        return f"https://{bucket}.{s3_host}/{key}"
    return url


def read_manifest(path: Path, s3_host: str) -> list[str]:
    urls: list[str] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        field = line.split("\t")[-1].split()[-1]  # last TSV / whitespace field
        field = normalise(field, s3_host)
        if field.startswith(("http://", "https://")):
            urls.append(field)
    # dedupe, keep order
    seen: set[str] = set()
    out = []
    for u in urls:
        if u not in seen:
            seen.add(u); out.append(u)
    return out


def fetch(url: str, out: Path, retries: int = 4) -> tuple[str, str]:
    if out.exists() and out.stat().st_size > 0:
        return ("skip", url)
    tmp = out.with_suffix(out.suffix + ".part")
    last = ""
    for _ in range(retries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "dataset-download/1.0"})
            with urllib.request.urlopen(req, timeout=120) as resp, tmp.open("wb") as fh:
                while chunk := resp.read(1 << 20):
                    fh.write(chunk)
            tmp.replace(out)
            return ("ok", url)
        except Exception as e:  # noqa: BLE001 — network best-effort
            last = f"{type(e).__name__}: {e}"
    tmp.unlink(missing_ok=True)
    return (f"FAIL {last}", url)


def main() -> None:
    manifest, dest, workers, s3_host = parse_args(sys.argv[1:])
    dest.mkdir(parents=True, exist_ok=True)
    urls = read_manifest(manifest, s3_host)
    jobs = [(u, dest / u.rsplit("/", 1)[-1].split("?")[0]) for u in urls]
    total = len(jobs)
    print(f"manifest={manifest} dest={dest} files={total} workers={workers}", flush=True)

    done = ok = skip = fail = 0
    failures: list[str] = []
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futs = [ex.submit(fetch, u, o) for u, o in jobs]
        for fut in as_completed(futs):
            status, url = fut.result()
            done += 1
            if status == "ok":
                ok += 1
            elif status == "skip":
                skip += 1
            else:
                fail += 1
                failures.append(f"{status}\t{url}")
            if done % 250 == 0 or done == total:
                print(f"  {done}/{total}  ok={ok} skip={skip} fail={fail}", flush=True)

    print(f"\nDONE files={total} ok={ok} skip={skip} fail={fail}", flush=True)
    fpath = dest / "download_failures.txt"
    if failures:
        fpath.write_text("\n".join(failures) + "\n", encoding="utf-8")
        print(f"  {fail} failures -> {fpath} (re-run to retry only those)", flush=True)
    elif fpath.exists():
        fpath.unlink()  # previous run's stale list; this run is clean


if __name__ == "__main__":
    main()
