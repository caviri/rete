#!/usr/bin/env python3
"""Harvest the ETH Research Collection OAI-PMH interface into gzipped page files.

Stdlib-only (runs in python:3.12-slim). Resumable: each response page is written
atomically to <out>/page_NNNNN.xml.gz; on restart the harvest continues after the
highest complete page by reconstructing DSpace's deterministic resumption token
(`<prefix>////<offset>`). During a run the token returned by the server is
followed verbatim.

Contexts (per https://unlimited.ethz.ch/spaces/RC/pages/194119646/OAI-PMH+interface):
  all_items — every item that passed review (306,835 on 2026-07-23)  <- we use this
  request   — only items with freely available full text (~114k)
  doi / openaire — subsets

Usage:
  python harvest_oai.py --context all_items --prefix oai_ethz \
      --out data/ethz-research-collection/raw/oai_ethz
  python harvest_oai.py --verb ListSets --out .../raw/sets
"""

import argparse
import gzip
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

BASE = "https://www.research-collection.ethz.ch/server/oai/{context}"
UA = "rete-dataset-harvester/1.0 (research use; contact: carlos.vivarrios@epfl.ch)"
PAGE_SIZE = 100  # DSpace fixed page size for ListRecords

TOKEN_RE = re.compile(r"<resumptionToken[^>]*>([^<]*)</resumptionToken>")
SIZE_RE = re.compile(r'completeListSize="(\d+)"')
ERROR_RE = re.compile(r'<error code="([^"]+)">([^<]*)')


def fetch(url: str, tries: int = 6, timeout: int = 180) -> bytes:
    delay = 5.0
    for attempt in range(1, tries + 1):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=timeout) as r:
                body = r.read()
            if b"</OAI-PMH>" not in body:
                raise ValueError("truncated response (no closing OAI-PMH tag)")
            return body
        except urllib.error.HTTPError as e:
            if e.code == 503:  # OAI politeness: honour Retry-After
                wait = int(e.headers.get("Retry-After") or delay)
                print(f"  503, retry in {wait}s", flush=True)
                time.sleep(wait)
                continue
            if attempt == tries:
                raise
            print(f"  HTTP {e.code} (attempt {attempt}/{tries}), retry in {delay:.0f}s", flush=True)
        except Exception as e:  # noqa: BLE001 — URLError, timeout, truncation
            if attempt == tries:
                raise
            print(f"  {type(e).__name__}: {e} (attempt {attempt}/{tries}), retry in {delay:.0f}s", flush=True)
        time.sleep(delay)
        delay = min(delay * 2, 120)
    raise RuntimeError("unreachable")


def save_page(out_dir: Path, page: int, body: bytes) -> None:
    dest = out_dir / f"page_{page:05d}.xml.gz"
    part = dest.with_suffix(".gz.part")
    with gzip.open(part, "wb", compresslevel=9) as f:
        f.write(body)
    part.rename(dest)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--context", default="all_items")
    ap.add_argument("--prefix", default="oai_ethz")
    ap.add_argument("--verb", default="ListRecords", choices=["ListRecords", "ListIdentifiers", "ListSets"])
    ap.add_argument("--out", required=True)
    ap.add_argument("--delay", type=float, default=1.0, help="seconds between requests")
    args = ap.parse_args()

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    for stale in out_dir.glob("*.part"):
        stale.unlink()

    base = BASE.format(context=args.context)

    # Resume: continue after the highest complete page already on disk.
    # NB: Path.stem on 'page_00134.xml.gz' is 'page_00134.xml' (strips one suffix
    # only), so parse the number from the name's digit run, not the stem.
    done = sorted(out_dir.glob("page_*.xml.gz"))
    page = len(done)
    if done:
        last_num = int(done[-1].name.split("_")[1].split(".")[0])
        if page != last_num + 1:
            print(f"ERROR: page files not contiguous ({len(done)} files, last is {done[-1].name})", flush=True)
            return 2
    if page == 0:
        q = {"verb": args.verb}
        if args.verb != "ListSets":
            q["metadataPrefix"] = args.prefix
        url = f"{base}?{urllib.parse.urlencode(q)}"
    else:
        # DSpace deterministic token: <prefix>////<offset> (empty for ListSets)
        prefix = args.prefix if args.verb != "ListSets" else ""
        token = f"{prefix}////{page * PAGE_SIZE}"
        url = f"{base}?verb={args.verb}&resumptionToken={urllib.parse.quote(token)}"
        print(f"resuming at page {page} (token {token})", flush=True)

    total = None
    t0 = time.time()
    pages_this_run = 0
    while True:
        body = fetch(url)
        text = body.decode("utf-8", errors="replace")
        err = ERROR_RE.search(text)
        if err:
            code, msg = err.groups()
            if code == "noRecordsMatch" and page > 0:
                print("noRecordsMatch past last page — done", flush=True)
                break
            print(f"OAI error {code}: {msg}", flush=True)
            return 1

        save_page(out_dir, page, body)
        page += 1
        pages_this_run += 1

        m = SIZE_RE.search(text)
        if m:
            total = int(m.group(1))
        tok = TOKEN_RE.search(text)
        token = tok.group(1).strip() if tok else ""
        if total and (pages_this_run == 1 or page % 25 == 0):
            expected = -(-total // PAGE_SIZE)
            rate = pages_this_run / max(time.time() - t0, 1e-9)
            eta_min = (expected - page) / max(rate, 1e-9) / 60
            print(f"page {page}/{expected}  ({page * PAGE_SIZE}/{total} records, ~{eta_min:.0f} min left)", flush=True)
        if not token:
            print(f"done: {page} pages", flush=True)
            break
        url = f"{base}?verb={args.verb}&resumptionToken={urllib.parse.quote(token)}"
        time.sleep(args.delay)
    return 0


if __name__ == "__main__":
    sys.exit(main())
