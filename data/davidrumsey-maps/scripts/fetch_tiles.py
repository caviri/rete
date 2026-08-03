#!/usr/bin/env python3
"""Parallel, resumable image-tier downloader driven by a <relpath>\t<url> TSV
(produced by extract_metadata.py). Uses persistent HTTP connections (one per
worker per host) — without keep-alive every request pays a fresh TLS handshake
and a 150k-file sweep balloons from hours to days.

    python fetch_tiles.py <manifest.tsv> <dest_dir> [--workers N] [--limit N]

Behaviour: skips existing non-empty files, writes .part then renames, retries
with backoff (reconnecting on stale/refused sockets), follows one redirect,
rejects non-image payloads (HTML error/captcha pages), logs misses to
<dest_dir>/download_failures.txt (re-run retries only those).
Stdlib only — runs in a plain python:3.12-slim container.
"""
from __future__ import annotations

import http.client
import sys
import threading
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

UA = "rete-dataset-harvester/1.0 (research; contact: carlosvivarrios@gmail.com)"
POLITE_SLEEP = 0.02
TL = threading.local()


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


def conn_for(host: str, fresh: bool = False) -> http.client.HTTPSConnection:
    conns = getattr(TL, "conns", None)
    if conns is None:
        conns = TL.conns = {}
    c = conns.get(host)
    if c is None or fresh:
        if c is not None:
            try:
                c.close()
            except OSError:
                pass
        c = conns[host] = http.client.HTTPSConnection(host, timeout=120)
    return c


def get(url: str, redirects: int = 2) -> bytes:
    """GET over a per-worker persistent connection; reconnect on stale socket."""
    u = urllib.parse.urlsplit(url)
    path = (u.path or "/") + (f"?{u.query}" if u.query else "")
    for attempt in (0, 1):  # second attempt on a fresh connection
        c = conn_for(u.netloc, fresh=attempt > 0)
        try:
            c.request("GET", path, headers={"User-Agent": UA, "Accept": "*/*"})
            r = c.getresponse()
            body = r.read()
            if r.status in (301, 302, 303, 307, 308) and redirects > 0:
                loc = r.getheader("Location", "")
                return get(urllib.parse.urljoin(url, loc), redirects - 1)
            if r.status != 200:
                raise ValueError(f"HTTP {r.status}")
            return body
        except (http.client.HTTPException, ConnectionError, TimeoutError, OSError):
            if attempt:
                raise
    raise RuntimeError("unreachable")


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
            body = get(url)
            if body.lstrip()[:1] in (b"<", b"{") or not body:
                raise ValueError("non-image payload (error/captcha page?)")
            tmp.write_bytes(body)
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

    t0 = time.time()
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
                rate = ok / max(time.time() - t0, 1)
                print(f"  {done}/{total}  ok={ok} skip={skip} fail={fail}  {rate:.1f}/s", flush=True)

    print(f"\nDONE files={total} ok={ok} skip={skip} fail={fail}", flush=True)
    fpath = dest / "download_failures.txt"
    if failures:
        fpath.write_text("\n".join(failures) + "\n", encoding="utf-8")
        print(f"  {fail} failures -> {fpath} (re-run to retry only those)", flush=True)
    elif fpath.exists():
        fpath.unlink()


if __name__ == "__main__":
    main()
