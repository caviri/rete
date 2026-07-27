#!/usr/bin/env python3
"""Harvest user/org profile metadata from the HF Hub API (stdlib-only, resumable).

For each name in raw/authors/authors_seed.tsv, GET
  /api/users/{name}/overview        (kind: user)
  /api/organizations/{name}/overview (kind: org)
trying the hinted kind first (default: user), falling back to the other.
Writes JSONL shards to raw/api/profiles/ and appends finished names to _done.txt,
so re-running continues where it left off.

Rate limit: anonymous 500 req/5min, authenticated 2,500 req/5min. Pass the token
via HF_TOKEN (never written to disk):
  MSYS_NO_PATHCONV=1 docker run -d --name hf-profiles -e HF_TOKEN=<token> \
    -v "D:/pro/rete:/w" -w //w python:3.12-slim \
    python data/hugging-face/scripts/harvest_profiles.py
"""
import argparse
import csv
import json
import os
import queue
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
API = "https://huggingface.co/api"


class RateLimiter:
    """Token bucket + global pause honored by all workers (429 → pause until reset)."""

    def __init__(self, rps):
        self.rps = rps
        self.lock = threading.Lock()
        self.next_at = time.monotonic()
        self.pause_until = 0.0

    def acquire(self):
        while True:
            with self.lock:
                now = time.monotonic()
                wait = max(self.pause_until - now, self.next_at - now)
                if wait <= 0:
                    self.next_at = max(self.next_at + 1.0 / self.rps, now)
                    return
            time.sleep(min(wait, 5.0))

    def pause(self, seconds):
        with self.lock:
            self.pause_until = max(self.pause_until, time.monotonic() + seconds)


def fetch(url, token, limiter):
    """GET url → (status, body_bytes, headers). Retries 5xx/network; honors 429."""
    for attempt in range(6):
        limiter.acquire()
        req = urllib.request.Request(url, headers={"User-Agent": "rete-hf-harvest/1.0"})
        if token:
            req.add_header("Authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.status, r.read(), dict(r.headers)
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return 404, e.read(), dict(e.headers)
            if e.code == 429:
                reset = 60
                rl = e.headers.get("RateLimit", "")
                if ";t=" in rl:
                    try:
                        reset = int(rl.split(";t=")[1].split(";")[0].strip('"'))
                    except ValueError:
                        pass
                limiter.pause(reset + 2)
                continue
            time.sleep(2 ** attempt)
        except (urllib.error.URLError, OSError, TimeoutError):
            time.sleep(2 ** attempt)
    return 0, b"", {}


def profile_one(name, hint, token, limiter):
    order = ["org", "user"] if hint == "org" else ["user", "org"]
    q = urllib.parse.quote(name, safe="")
    for kind in order:
        path = "users" if kind == "user" else "organizations"
        status, body, _ = fetch(f"{API}/{path}/{q}/overview", token, limiter)
        if status == 200:
            try:
                data = json.loads(body)
            except json.JSONDecodeError:
                continue
            return {"name": name, "kind": kind, "status": "ok",
                    "fetched_at": datetime.now(timezone.utc).isoformat(), "data": data}
        if status == 0:
            return {"name": name, "kind": None, "status": "error",
                    "fetched_at": datetime.now(timezone.utc).isoformat()}
    return {"name": name, "kind": None, "status": "gone",
            "fetched_at": datetime.now(timezone.utc).isoformat()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", default=os.path.join(BASE, "raw", "authors", "authors_seed.tsv"))
    ap.add_argument("--out", default=os.path.join(BASE, "raw", "api", "profiles"))
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--rps", type=float, default=None,
                    help="default: 7.5 with HF_TOKEN, 1.4 anonymous")
    ap.add_argument("--limit", type=int, default=0, help="stop after N names (smoke test)")
    args = ap.parse_args()

    token = os.environ.get("HF_TOKEN", "").strip()
    rps = args.rps if args.rps else (7.5 if token else 1.4)
    os.makedirs(args.out, exist_ok=True)

    done_path = os.path.join(args.out, "_done.txt")
    done = set()
    if os.path.exists(done_path):
        with open(done_path, encoding="utf-8") as f:
            done = {line.rstrip("\n") for line in f}

    todo = []
    with open(args.seed, encoding="utf-8") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            if row["name"] and row["name"] not in done:
                todo.append((row["name"], row["kind_hint"]))
    if args.limit:
        todo = todo[: args.limit]
    total = len(todo)
    print(f"profiles: {len(done):,} done, {total:,} to go, rps={rps}, "
          f"auth={'yes' if token else 'NO (anonymous)'}", flush=True)
    if not todo:
        return

    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S")
    shard = open(os.path.join(args.out, f"profiles-{run_id}.jsonl"), "a", encoding="utf-8")
    done_f = open(done_path, "a", encoding="utf-8")
    wlock = threading.Lock()
    limiter = RateLimiter(rps)
    q = queue.Queue()
    for item in todo:
        q.put(item)
    counter = {"n": 0}
    t0 = time.monotonic()

    def worker():
        while True:
            try:
                name, hint = q.get_nowait()
            except queue.Empty:
                return
            rec = profile_one(name, hint, token, limiter)
            with wlock:
                shard.write(json.dumps(rec, ensure_ascii=False) + "\n")
                if rec["status"] != "error":       # errors stay in todo for the next run
                    done_f.write(name + "\n")
                counter["n"] += 1
                n = counter["n"]
                if n % 500 == 0:
                    shard.flush()
                    done_f.flush()
                    rate = n / (time.monotonic() - t0)
                    eta_h = (total - n) / rate / 3600 if rate else 0
                    print(f"  {n:,}/{total:,}  {rate:.1f}/s  eta {eta_h:.1f}h", flush=True)

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
    print(f"finished {counter['n']:,} names in {(time.monotonic()-t0)/60:.1f} min", flush=True)


if __name__ == "__main__":
    sys.exit(main())
