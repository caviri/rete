#!/usr/bin/env python3
"""Parallel, resumable image-tier downloader driven by a <relpath>\t<url> TSV
(produced by extract_metadata.py). Like skills/dataset-download/scripts/
fetch_urls.py, but preserves the collection's directory layout — Size3/4 URLs
all end in `srvr?mediafile=...`, so naming by URL basename would collide.

    python fetch_tiles.py <manifest.tsv> <dest_dir> [--workers N] [--limit N]

Behaviour: skips existing non-empty files, writes .part then renames, retries
with backoff, logs misses to <dest_dir>/download_failures.txt (re-run retries
only those). Rejects non-image payloads (HTML error/captcha pages).
Stdlib only — runs in a plain python:3.12-slim container.
"""
from __future__ import annotations

import sys
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

UA = {"User-Agent": "rete-dataset-harvester/1.0 (research; contact: carlosvivarrios@gmail.com)"}
POLITE_SLEEP = 0.05


def parse_args(argv: list[str]):
    workers, limit = 8, 0
    pos: list[str] = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--workers":
            workers = int(argv[i + 1]); i += 2; continue
        if a == "--limit":
            limit = int(argv[i + 1]); i += 2; continue
        pos.append(a); i += 1
    if len(pos) != 2:
        sys.exit("usage: fetch_tiles.py <manifest.tsv> <dest_dir> [--workers N] [--limit N]")
    return Path(pos[0]), Path(pos[1]), workers, limit


def fetch(rel: str, url: str, root: Path, retries: int = 5) -> tuple[str, str]:
    dest = root / rel
    if dest.exists() and dest.stat().st_size > 0:
        return ("skip", rel)
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    last = ""
    for attempt in range(retries):
        try:
            time.sleep(POLITE_SLEEP)
            req = urllib.request.Request(url, headers=UA)
            with urllib.request.urlopen(req, timeout=180) as resp:
                head = resp.read(16)
                if head.lstrip()[:1] in (b"<", b"{"):
                    raise ValueError("non-image payload (error/captcha page?)")
                with tmp.open("wb") as fh:
                    fh.write(head)
                    while chunk := resp.read(1 << 20):
                        fh.write(chunk)
            tmp.replace(dest)
            return ("ok", rel)
        except Exception as e:  # noqa: BLE001 — network best-effort
            last = f"{type(e).__name__}: {e}"
            time.sleep(min(2 ** attempt, 30))
    tmp.unlink(missing_ok=True)
    return (f"FAIL {last}\t{url}", rel)


def main() -> None:
    manifest, dest, workers, limit = parse_args(sys.argv[1:])
    rows: list[tuple[str, str]] = []
    for line in manifest.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        rel, _, url = line.partition("\t")
        if url.startswith(("http://", "https://")):
            rows.append((rel.strip(), url.strip()))
    if limit:
        rows = rows[:limit]
    dest.mkdir(parents=True, exist_ok=True)
    total = len(rows)
    print(f"manifest={manifest} dest={dest} files={total} workers={workers}", flush=True)

    done = ok = skip = fail = 0
    failures: list[str] = []
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futs = [ex.submit(fetch, rel, url, dest) for rel, url in rows]
        for fut in as_completed(futs):
            status, rel = fut.result()
            done += 1
            if status == "ok":
                ok += 1
            elif status == "skip":
                skip += 1
            else:
                fail += 1
                failures.append(f"{status}\t{rel}")
            if done % 1000 == 0 or done == total:
                print(f"  {done}/{total}  ok={ok} skip={skip} fail={fail}", flush=True)

    print(f"\nDONE files={total} ok={ok} skip={skip} fail={fail}", flush=True)
    fpath = dest / "download_failures.txt"
    if failures:
        fpath.write_text("\n".join(failures) + "\n", encoding="utf-8")
        print(f"  {fail} failures -> {fpath} (re-run to retry only those)", flush=True)
    elif fpath.exists():
        fpath.unlink()


if __name__ == "__main__":
    main()
