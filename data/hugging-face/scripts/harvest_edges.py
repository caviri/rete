#!/usr/bin/env python3
"""Harvest the social graph edges from the HF Hub API (stdlib-only, resumable).

Reads the profiles harvested by harvest_profiles.py and, per --kind:
  members    orgs            → GET /api/organizations/{o}/members    (org→user)
  followers  users AND orgs  → GET /api/users/{u}/followers | /api/organizations/{o}/followers
  following  users           → GET /api/users/{u}/following          (user→user/org)
All endpoints paginate with ?limit=100 and a Link: rel="next" cursor.

Writes JSONL to raw/api/<kind>/ and appends finished names to _done.txt (resumable).
Usage (in Docker, token via -e HF_TOKEN):
  python data/hugging-face/scripts/harvest_edges.py --kind members
"""
import argparse
import glob
import json
import os
import queue
import re
import sys
import threading
import time
from datetime import datetime, timezone

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harvest_profiles import BASE, API, RateLimiter, fetch  # noqa: E402

import urllib.parse


def walk(url, token, limiter, max_pages):
    """Follow cursor pages. Returns (items, status): ok | error | truncated."""
    items, pages = [], 0
    while url:
        if pages >= max_pages:
            return items, "truncated"
        status, body, headers = fetch(url, token, limiter)
        if status != 200:
            return items, "error"
        try:
            items.extend(json.loads(body))
        except json.JSONDecodeError:
            return items, "error"
        pages += 1
        link = headers.get("Link", "")
        m = re.search(r'<([^>]+)>;\s*rel="next"', link)
        url = m.group(1) if m else None
    return items, "ok"


def load_profiles(profiles_dir):
    """name → (kind, numFollowers, numFollowing, numUsers) from the newest record."""
    out = {}
    for path in sorted(glob.glob(os.path.join(profiles_dir, "profiles-*.jsonl"))):
        with open(path, encoding="utf-8") as f:
            for line in f:
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if rec.get("status") != "ok":
                    continue
                d = rec.get("data", {})
                out[rec["name"]] = (rec["kind"], d.get("numFollowers", 0),
                                    d.get("numFollowing", 0), d.get("numUsers", 0))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kind", required=True, choices=["members", "followers", "following"])
    ap.add_argument("--profiles", default=os.path.join(BASE, "raw", "api", "profiles"))
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--rps", type=float, default=None)
    ap.add_argument("--limit", type=int, default=0, help="stop after N names (smoke test)")
    ap.add_argument("--max-pages", type=int, default=2000,
                    help="cap pages per name (2000 = 200k edges); aborted walks are logged")
    args = ap.parse_args()

    token = os.environ.get("HF_TOKEN", "").strip()
    rps = args.rps if args.rps else (7.5 if token else 1.4)
    out_dir = os.path.join(BASE, "raw", "api", args.kind)
    os.makedirs(out_dir, exist_ok=True)

    done_path = os.path.join(out_dir, "_done.txt")
    done = set()
    if os.path.exists(done_path):
        with open(done_path, encoding="utf-8") as f:
            done = {line.rstrip("\n") for line in f}

    profiles = load_profiles(args.profiles)
    todo = []
    for name, (kind, n_followers, n_following, n_users) in profiles.items():
        if name in done:
            continue
        if args.kind == "members" and kind == "org" and n_users > 0:
            todo.append((name, kind, n_users))
        elif args.kind == "followers" and n_followers > 0:
            todo.append((name, kind, n_followers))
        elif args.kind == "following" and kind == "user" and n_following > 0:
            todo.append((name, kind, n_following))
    todo.sort(key=lambda t: -t[2])          # biggest accounts first
    if args.limit:
        todo = todo[: args.limit]
    total_names = len(todo)
    total_edges = sum(t[2] for t in todo)
    print(f"{args.kind}: {len(done):,} done, {total_names:,} names to walk "
          f"(~{total_edges:,} edges expected), rps={rps}, "
          f"auth={'yes' if token else 'NO (anonymous)'}", flush=True)
    if not todo:
        return

    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S")
    shard = open(os.path.join(out_dir, f"{args.kind}-{run_id}.jsonl"), "a", encoding="utf-8")
    done_f = open(done_path, "a", encoding="utf-8")
    wlock = threading.Lock()
    limiter = RateLimiter(rps)
    q = queue.Queue()
    for item in todo:
        q.put(item)
    counter = {"names": 0, "edges": 0}
    t0 = time.monotonic()

    def start_url(name, kind):
        qn = urllib.parse.quote(name, safe="")
        if args.kind == "members":
            return f"{API}/organizations/{qn}/members?limit=100"
        root = "organizations" if kind == "org" else "users"
        return f"{API}/{root}/{qn}/{args.kind}?limit=100"

    def worker():
        while True:
            try:
                name, kind, expected = q.get_nowait()
            except queue.Empty:
                return
            rows, wstatus = walk(start_url(name, kind), token, limiter, args.max_pages)
            now = datetime.now(timezone.utc).isoformat()
            with wlock:
                for it in rows:
                    edge = {"src": name, "src_kind": kind, "kind": args.kind,
                            "fetched_at": now, "target": it}
                    shard.write(json.dumps(edge, ensure_ascii=False) + "\n")
                counter["edges"] += len(rows)
                if wstatus == "truncated":      # capped by --max-pages: done, but on record
                    shard.write(json.dumps({"src": name, "src_kind": kind,
                                            "kind": f"{args.kind}_truncated",
                                            "fetched_at": now, "expected": expected,
                                            "got": len(rows)}) + "\n")
                    print(f"  TRUNCATED {name}: {len(rows)}/{expected}", flush=True)
                if wstatus != "error":          # errored walks retry on the next run
                    done_f.write(name + "\n")
                counter["names"] += 1
                n = counter["names"]
                if n % 100 == 0:
                    shard.flush(); done_f.flush()
                    rate = counter["edges"] / (time.monotonic() - t0)
                    print(f"  {n:,}/{total_names:,} names, {counter['edges']:,} edges "
                          f"({rate:.0f} edges/s)", flush=True)

    threads = [threading.Thread(target=worker, daemon=True) for _ in range(args.workers)]
    for t in threads:
        t.start()
    try:
        for t in threads:
            t.join()
    finally:
        with wlock:
            shard.flush(); shard.close()
            done_f.flush(); done_f.close()
    print(f"finished: {counter['names']:,} names, {counter['edges']:,} edges "
          f"in {(time.monotonic()-t0)/60:.1f} min", flush=True)


if __name__ == "__main__":
    sys.exit(main())
