#!/usr/bin/env python3
"""Full mirror of the Renouvaud / BCU catalogue (Alma SRU, zone 41BCULAUSA_LIB).

Alma SRU caps retrieval at 10 000 records/query AND each request costs ~3 s, so a
single stream over 71k pages would take ~2.5 days. We therefore:

  1) PLAN — recursively bisect the numeric `alma.mms_id` space into leaf ranges of
     <=10 000 records. Every record has a unique 18-digit id and the integer
     partition (lo,mid) | [mid,hi) is exact, so leaves are provably complete with
     no undated/untitled coverage gaps. Counting is done concurrently (BFS levels).
  2) HARVEST — fetch every leaf's pages with a thread pool (WORKERS concurrent
     requests, ~4 req/s aggregate). One process, one JSONL writer under a lock
     (no cross-process file races). Resumable at leaf granularity.

Usage:
  python harvest_renouvaud.py                 # full mirror (plan+harvest), resume
  python harvest_renouvaud.py --workers 12
  python harvest_renouvaud.py --plan-only
  python harvest_renouvaud.py --max-leaves 3  # smoke test
  python harvest_renouvaud.py --restart
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import gzip
import json
import re
import sys
import threading
import urllib.parse
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from http_util import Fetcher  # noqa: E402
import marc  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
SRU = "https://renouvaud1.alma.exlibrisgroup.com/view/sru/41BCULAUSA_LIB"
CAP = 10000
PAGE = 50
LO_INIT = 990000000000000000
HI_INIT = 992000000000000000
TOTAL_HINT = 3551404
_NUM_RE = re.compile(rb"<numberOfRecords>(\d+)</numberOfRecords>")


def now_iso():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def q_range(lo, hi):
    return f"alma.mms_id>{lo} and alma.mms_id<{hi}"


def sru_url(query, start=1, maxrec=1):
    return (f"{SRU}?version=1.2&operation=searchRetrieve"
            f"&query={urllib.parse.quote(query)}&maximumRecords={maxrec}&startRecord={start}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-dir", default=str(REPO / "data" / "bcul"))
    ap.add_argument("--workers", type=int, default=12)
    ap.add_argument("--max-leaves", type=int, default=0)
    ap.add_argument("--plan-only", action="store_true")
    ap.add_argument("--restart", action="store_true")
    args = ap.parse_args()

    base = Path(args.base_dir)
    raw_dir = base / "raw" / "renouvaud" / "sru"
    raw_dir.mkdir(parents=True, exist_ok=True)
    norm_path = base / "normalized" / "renouvaud.jsonl"
    leaves_path = base / "state" / "renouvaud_leaves.json"
    state_path = base / "state" / "renouvaud.json"
    log_path = base / "logs" / "renouvaud.log"
    gap_path = base / "logs" / "renouvaud_gaps.log"
    for p in (norm_path.parent, state_path.parent, log_path.parent):
        p.mkdir(parents=True, exist_ok=True)

    log_lock = threading.Lock()

    def log(msg):
        line = f"[{now_iso()}] {msg}"
        with log_lock:
            print(line, flush=True)
            with open(log_path, "a", encoding="utf-8") as fh:
                fh.write(line + "\n")

    f = Fetcher(rate=0, retries=6, timeout=120)  # no artificial throttle; concurrency paces us

    def count(query):
        data, _, _ = f.get(sru_url(query, 1, 1), accept="application/xml")
        m = _NUM_RE.search(data or b"")
        if m:
            return int(m.group(1))
        if b"<diagnostic" in (data or b""):
            data, _, _ = f.get(sru_url(query, 1, 1), accept="application/xml")
            m = _NUM_RE.search(data or b"")
            if m:
                return int(m.group(1))
        return 0

    def fetch_page(query, start):
        for _ in range(3):
            data, _, _ = f.get(sru_url(query, start, PAGE), accept="application/xml")
            if data is None:
                return None
            if (b"<record" in data and b"<recordData" in data) or b"<diagnostic" not in data:
                return data
        return data

    # ---------------------------------------------------------------- PLAN
    if leaves_path.exists() and not args.restart:
        leaves = json.loads(leaves_path.read_text(encoding="utf-8"))
        log(f"Loaded plan: {len(leaves)} leaves.")
    else:
        log("Planning: bisecting mms_id space (concurrent counts)...")
        leaves = []
        frontier = [(LO_INIT, HI_INIT)]
        with cf.ThreadPoolExecutor(max_workers=args.workers) as ex:
            level = 0
            while frontier:
                counts = list(ex.map(lambda r: count(q_range(*r)), frontier))
                nxt = []
                for (lo, hi), n in zip(frontier, counts):
                    if n == 0:
                        continue
                    if n <= CAP:
                        leaves.append([lo, hi, n])
                    else:
                        mid = (lo + hi) // 2
                        nxt.append((lo, mid))
                        nxt.append((mid - 1, hi))
                level += 1
                planned = sum(x[2] for x in leaves)
                log(f"  plan L{level}: {len(leaves)} leaves ({planned} recs), {len(nxt)} ranges to expand")
                frontier = nxt
        leaves.sort()
        leaves_path.write_text(json.dumps(leaves), encoding="utf-8")
        total_planned = sum(x[2] for x in leaves)
        log(f"Plan complete: {len(leaves)} leaves covering {total_planned} records "
            f"(hint {TOTAL_HINT}, delta {TOTAL_HINT - total_planned}).")

    if args.plan_only:
        return 0

    # ---------------------------------------------------------------- HARVEST
    state = {"done_leaves": [], "records": 0, "started": now_iso(), "done": False}
    if state_path.exists() and not args.restart:
        state = json.loads(state_path.read_text(encoding="utf-8"))
    else:
        norm_path.write_text("", encoding="utf-8")
    done = set(tuple(x) for x in state["done_leaves"])
    log(f"Harvesting {len(leaves) - len(done)} remaining leaves "
        f"({len(done)} already done, {state['records']} records) with {args.workers} workers.")

    norm_fh = open(norm_path, "a", encoding="utf-8")
    write_lock = threading.Lock()
    counter = {"leaves": len(done), "records": state["records"]}

    def do_leaf(leaf):
        lo, hi, n = leaf
        if (lo, hi) in done:
            return 0
        query = q_range(lo, hi)
        recs, raw, got = [], [], 0
        harvested_at = now_iso()
        start, consec_empty = 1, 0
        while start <= n:
            data = fetch_page(query, start)
            if data is None:
                consec_empty += 1
                if consec_empty >= 4:
                    break
                start += PAGE
                continue
            raw.append(data)
            page_n = 0
            for m in marc.iter_marc_records(data):
                rec = marc.normalize(m, "renouvaud")
                rec["harvested_at"] = harvested_at
                recs.append(rec)
                page_n += 1
                got += 1
            start += PAGE
            consec_empty = 0 if page_n else consec_empty + 1
            if consec_empty >= 4:
                break
        (raw_dir / f"mms_{lo}_{hi}.xml.gz").write_bytes(
            gzip.compress(b"\n<!-- PAGE -->\n".join(raw)))
        with write_lock:
            for r in recs:
                norm_fh.write(json.dumps(r, ensure_ascii=False) + "\n")
            norm_fh.flush()
            state["done_leaves"].append([lo, hi])
            state["records"] += got
            state_path.write_text(json.dumps(state), encoding="utf-8")
            counter["leaves"] += 1
            counter["records"] += got
            if got < n:
                with open(gap_path, "a", encoding="utf-8") as gh:
                    gh.write(f"[{now_iso()}] SHORTFALL {query}: expected {n} got {got}\n")
            if counter["leaves"] % 10 == 0 or got < n:
                pct = 100 * counter["records"] / TOTAL_HINT
                log(f"leaf {counter['leaves']}/{len(leaves)} ({lo},{hi}) n={n} got={got} | "
                    f"total {counter['records']} (~{pct:.1f}%) req#{f.n_requests}")
        return got

    todo = [lf for lf in leaves if tuple(lf[:2]) not in done]
    if args.max_leaves:
        todo = todo[:args.max_leaves]
    try:
        with cf.ThreadPoolExecutor(max_workers=args.workers) as ex:
            list(ex.map(do_leaf, todo))
    finally:
        norm_fh.close()

    if not args.max_leaves and len(state["done_leaves"]) >= len(leaves):
        state["done"] = True
        state_path.write_text(json.dumps(state), encoding="utf-8")
        log(f"DONE. Renouvaud mirror complete: {state['records']} records in {len(leaves)} leaves.")
    else:
        log(f"Paused: {counter['leaves']}/{len(leaves)} leaves, {counter['records']} records.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
