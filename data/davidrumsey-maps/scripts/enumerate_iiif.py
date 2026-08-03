#!/usr/bin/env python3
"""Enumerate every item in the David Rumsey Historical Map Collection (LUNA)
via IIIF Collection pagination — the robots-clean machine lane.

    python enumerate_iiif.py [--token TOKEN] [--out DIR] [--workers N]

LUNA exposes the whole collection (150,017 items as of 2026-08) as a paged
IIIF Collection:

    https://www.davidrumsey.com/luna/servlet/iiif/collection/s/<token>        # -> total, first
    https://www.davidrumsey.com/luna/servlet/iiif/collection/s/<token>/<page> # 10 manifests/page

The <token> is minted per LUNA "search session". Without --token the script
mints one with a single call to the LUNA JSON API (/luna/servlet/as/search),
reading the `iiifCollection` URL out of the response. If that call returns the
reCAPTCHA interstitial instead of JSON, open davidrumsey.com in a browser,
solve it once, and pass the token from the viewer's IIIF link via --token.

Outputs (in --out, default data/davidrumsey-maps/raw):
    enum_pages.jsonl    resumable per-page state ({"page":N,"items":[{id,label}]})
    items_index.tsv     <id>\t<label>  — one line per item, page order
    manifest_urls.txt   one IIIF manifest URL per item

Resume-safe: re-run and it only fetches pages missing from enum_pages.jsonl.
Stdlib only — runs in a plain python:3.12-slim container.
"""
from __future__ import annotations

import json
import sys
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from threading import Lock

BASE = "https://www.davidrumsey.com/luna/servlet/iiif/collection/s/{token}"
MINT_URL = ("https://www.davidrumsey.com/luna/servlet/as/search"
            "?q=&lc=RUMSEY~8~1&pgs=1&res=1&pos=0")
UA = {"User-Agent": "rete-dataset-harvester/1.0 (research; contact: carlosvivarrios@gmail.com)"}
POLITE_SLEEP = 0.15  # per request, per worker


def parse_args(argv: list[str]):
    token, out, workers = "", Path("data/davidrumsey-maps/raw"), 4
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--token":
            token = argv[i + 1]; i += 2; continue
        if a == "--out":
            out = Path(argv[i + 1]); i += 2; continue
        if a == "--workers":
            workers = int(argv[i + 1]); i += 2; continue
        sys.exit(f"unknown arg: {a}")
    return token, out, workers


def get_json(url: str, retries: int = 6):
    last = ""
    for attempt in range(retries):
        try:
            time.sleep(POLITE_SLEEP)
            with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
                body = r.read().decode("utf-8", "replace")
            if not body.lstrip().startswith(("{", "[")):
                raise ValueError("non-JSON response (captcha wall?)")
            return json.loads(body)
        except Exception as e:  # noqa: BLE001 — network best-effort
            last = f"{type(e).__name__}: {e}"
            time.sleep(min(2 ** attempt, 30))
    raise RuntimeError(f"{url} -> {last}")


def mint_token() -> str:
    d = get_json(MINT_URL, retries=2)
    col = d.get("iiifCollection", "")
    tok = col.rstrip("/").rsplit("/", 1)[-1] if "/collection/s/" in col else ""
    if not tok:
        sys.exit("could not mint a session token (captcha?) — pass --token from a browser session")
    print(f"minted IIIF collection token: {tok}", flush=True)
    return tok


def item_of(man: dict) -> dict:
    mid = man.get("@id", "")
    # .../iiif/m/<ID>/manifest
    iid = mid.split("/iiif/m/", 1)[-1].rsplit("/manifest", 1)[0] if "/iiif/m/" in mid else mid
    label = " ".join(str(man.get("label", "")).split())
    return {"id": iid, "label": label}


def main() -> None:
    token, out, workers = parse_args(sys.argv[1:])
    out.mkdir(parents=True, exist_ok=True)
    if not token:
        token = mint_token()
    base = BASE.format(token=token)

    head = get_json(base)
    total = int(head.get("total", 0))
    page0 = get_json(f"{base}/0")
    psize = max(1, len(page0.get("manifests", [])))
    npages = -(-total // psize)
    print(f"total={total} page_size={psize} pages={npages}", flush=True)

    state = out / "enum_pages.jsonl"
    done: set[int] = set()
    if state.exists():
        for line in state.open(encoding="utf-8"):
            try:
                done.add(json.loads(line)["page"])
            except Exception:  # noqa: BLE001 — tolerate a torn tail line
                pass
    print(f"resume: {len(done)} pages already done", flush=True)

    lock = Lock()
    written = len(done)
    with state.open("a", encoding="utf-8") as sf:
        def do_page(n: int):
            d = page0 if n == 0 else get_json(f"{base}/{n}")
            items = [item_of(m) for m in d.get("manifests", [])]
            with lock:
                sf.write(json.dumps({"page": n, "items": items}, ensure_ascii=False) + "\n")
                sf.flush()
            return n

        todo = [n for n in range(npages) if n not in done]
        with ThreadPoolExecutor(max_workers=workers) as ex:
            futs = [ex.submit(do_page, n) for n in todo]
            for fut in as_completed(futs):
                fut.result()  # propagate failures loudly
                written += 1
                if written % 200 == 0 or written == npages:
                    print(f"  pages {written}/{npages}", flush=True)

    # consolidate: page-ordered index + manifest URL list
    pages: dict[int, list[dict]] = {}
    for line in state.open(encoding="utf-8"):
        try:
            d = json.loads(line)
            pages[d["page"]] = d["items"]
        except Exception:  # noqa: BLE001
            pass
    ids: list[dict] = []
    seen: set[str] = set()
    for n in sorted(pages):
        for it in pages[n]:
            if it["id"] and it["id"] not in seen:
                seen.add(it["id"])
                ids.append(it)

    with (out / "items_index.tsv").open("w", encoding="utf-8") as f:
        for it in ids:
            f.write(f"{it['id']}\t{it['label']}\n")
    with (out / "manifest_urls.txt").open("w", encoding="utf-8") as f:
        for it in ids:
            f.write(f"https://www.davidrumsey.com/luna/servlet/iiif/m/{it['id']}/manifest\n")

    print(f"DONE items={len(ids)} (expected {total}) -> items_index.tsv, manifest_urls.txt", flush=True)
    if len(ids) != total:
        print("WARNING: item count != collection total — re-run to fill gaps, "
              "or the collection changed size mid-harvest", flush=True)


if __name__ == "__main__":
    main()
