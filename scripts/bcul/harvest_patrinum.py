#!/usr/bin/env python3
"""Full OAI-PMH harvest of Patrinum (patrinum.ch / TIND) -> raw pages + normalized JSONL.

368k records, MARCXML, 100/page with resumption tokens. Resumable: the next token,
page number and counts are checkpointed after every page, so re-running continues.

Usage:
  python harvest_patrinum.py                 # full harvest (all sets), resume if state exists
  python harvest_patrinum.py --set BCUArchives
  python harvest_patrinum.py --max-pages 2   # smoke test
  python harvest_patrinum.py --restart       # ignore checkpoint
"""
from __future__ import annotations

import argparse
import gzip
import json
import sys
import time
import urllib.parse
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from http_util import Fetcher  # noqa: E402
import marc  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
OAI = "https://patrinum.ch/oai2d"


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-dir", default=str(REPO / "data" / "bcul"))
    ap.add_argument("--set", default=None, help="OAI setSpec (default: all records)")
    ap.add_argument("--rate", type=float, default=2.5)
    ap.add_argument("--max-pages", type=int, default=0, help="0 = unlimited")
    ap.add_argument("--restart", action="store_true")
    args = ap.parse_args()

    base = Path(args.base_dir)
    raw_dir = base / "raw" / "patrinum" / "oai"
    raw_dir.mkdir(parents=True, exist_ok=True)
    norm_path = base / "normalized" / "patrinum.jsonl"
    norm_path.parent.mkdir(parents=True, exist_ok=True)
    state_path = base / "state" / ("patrinum.json" if not args.set else f"patrinum.{args.set}.json")
    state_path.parent.mkdir(parents=True, exist_ok=True)
    log_path = base / "logs" / "patrinum.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)

    def log(msg: str) -> None:
        line = f"[{now_iso()}] {msg}"
        print(line, flush=True)
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(line + "\n")

    f = Fetcher(rate=args.rate)

    # ---- resume state
    state = {"token": None, "page": 0, "records": 0, "started": now_iso(), "done": False}
    if state_path.exists() and not args.restart:
        state = json.loads(state_path.read_text(encoding="utf-8"))
        if state.get("done"):
            log(f"Patrinum harvest already complete ({state['records']} records). Use --restart to redo.")
            return 0
        log(f"Resuming Patrinum from page {state['page']} ({state['records']} records so far)")
    else:
        # fresh start truncates the normalized file
        norm_path.write_text("", encoding="utf-8")

    norm_fh = open(norm_path, "a", encoding="utf-8")
    total_hint = None
    try:
        while True:
            if state["token"]:
                url = f"{OAI}?verb=ListRecords&resumptionToken={urllib.parse.quote(state['token'])}"
            elif args.set:
                url = f"{OAI}?verb=ListRecords&metadataPrefix=marcxml&set={urllib.parse.quote(args.set)}"
            else:
                url = f"{OAI}?verb=ListRecords&metadataPrefix=marcxml"

            data, _ctype, status = f.get(url, accept="application/xml")
            if data is None:
                log(f"HTTP {status} on page {state['page'] + 1}; stopping")
                break

            # raw page
            page_no = state["page"] + 1
            (raw_dir / f"page_{page_no:05d}.xml.gz").write_bytes(gzip.compress(data))

            # parse + normalize
            n_page = 0
            harvested_at = now_iso()
            for hdr, m in marc.iter_oai_records(data):
                if m is None:
                    continue  # deleted or metadata-less
                rec = marc.normalize(m, "patrinum", oai_header=hdr)
                rec["harvested_at"] = harvested_at
                norm_fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
                n_page += 1
            norm_fh.flush()

            tok = marc.get_resumption_token(data)
            # completeListSize hint
            if total_hint is None:
                import re
                mt = re.search(rb'completeListSize="(\d+)"', data)
                if mt:
                    total_hint = int(mt.group(1))

            state["page"] = page_no
            state["records"] += n_page
            state["token"] = tok
            state["updated"] = now_iso()
            if not tok:
                state["done"] = True
            state_path.write_text(json.dumps(state), encoding="utf-8")

            pct = f" (~{100 * state['records'] // total_hint}%)" if total_hint else ""
            log(f"page {page_no}: +{n_page} recs, total {state['records']}{pct}, req#{f.n_requests}")

            if not tok:
                log(f"DONE. {state['records']} Patrinum records harvested.")
                break
            if args.max_pages and page_no >= args.max_pages:
                log(f"Reached --max-pages {args.max_pages}; pausing (resume later).")
                break
    finally:
        norm_fh.close()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
