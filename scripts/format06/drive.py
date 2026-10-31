"""Run remote jobs with the 0.4.2 CLI through the WS20 latency proxy, one cold process per run, and keep the proxy's
per-request log (arrival time + Range) between start/end marks, so every range read of a job is recorded.

usage: python drive.py JOBS.json OUT.jsonl MODE [--only id,id] [--runs N]
  MODE: nocache (RETE_BLOCK_KB=0) | cache (auto block size, the default client)
Each job: {"id", "file" (path under the nginx root), "kind": card|sparql|prefix|contains|open, "args": [...]}.
"""
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

PROXY_HOST = os.environ.get("PROXY_HOST", "http://127.0.0.1:18080")
PROXY_NET = os.environ.get("PROXY_NET", "http://ws20-proxy:8080")
IMAGE = "ghcr.io/caviri/rete-cli:0.4.2"
DOCKER = os.environ.get("DOCKER", "docker")


def mark(tag):
    urllib.request.urlopen(f"{PROXY_HOST}/__mark__/{tag}", timeout=30).read()


def cli_args(job, url):
    k, a = job["kind"], job.get("args", [])
    if k == "card":
        return ["card-url", url, "--json"]
    if k == "sparql":
        return ["sparql-url", url, a[0], "--json"] + a[1:]
    if k == "prefix":
        return ["search-url", url, a[0], "--limit", "1000", "--json"]
    if k == "contains":
        return ["search-url", url] + a + ["--limit", "100000", "--json"]
    raise SystemExit(k)


def main():
    jobs = json.load(open(sys.argv[1]))
    out = open(sys.argv[2], "a")
    mode = sys.argv[3]
    only = None
    if "--only" in sys.argv:
        only = set(sys.argv[sys.argv.index("--only") + 1].split(","))
    runs = int(sys.argv[sys.argv.index("--runs") + 1]) if "--runs" in sys.argv else 1
    env = ["-e", "RETE_HTTP_TRACE=1"] + (["-e", "RETE_BLOCK_KB=0"] if mode == "nocache" else [])
    for job in jobs:
        if only and job["id"] not in only:
            continue
        for r in range(runs):
            url = f"{PROXY_NET}/{job['file']}"
            cmd = [DOCKER, "run", "--rm", "--network", "ws20-net", "--memory", "4g", "--memory-swap", "4g",
                   "--cpus", "4"] + env + [IMAGE] + cli_args(job, url)
            tag = f"{mode}/{job['id']}/{r}"
            mark(tag + "/start")
            t = time.time()
            p = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace",
                               timeout=job.get("timeout", 900))
            wall = time.time() - t
            mark(tag + "/end")
            m = re.search(r"fetched (\d+) (?:of \d+ )?bytes in (\d+) range request", p.stderr)
            rows = None
            try:
                d = json.loads(p.stdout)
                if isinstance(d, dict) and "results" in d:
                    rows = len(d["results"]["bindings"])
                elif isinstance(d, dict) and "boolean" in d:
                    rows = 1
                elif isinstance(d, dict) and "matches" in d:
                    rows = len(d["matches"])
                elif isinstance(d, list):
                    rows = len(d)
                else:
                    rows = 1
            except Exception:  # noqa: BLE001
                rows = len([ln for ln in p.stdout.splitlines() if ln.strip()])
            trace = [ln for ln in p.stderr.splitlines() if ln.startswith("[http]")]
            rec = {"mode": mode, "id": job["id"], "file": job["file"], "kind": job["kind"], "run": r,
                   "rc": p.returncode, "wall": round(wall, 3), "rows": rows,
                   "cli_bytes": int(m.group(1)) if m else None, "cli_requests": int(m.group(2)) if m else None,
                   "trace": trace, "err": "" if p.returncode == 0 else p.stderr[-600:]}
            out.write(json.dumps(rec) + "\n")
            out.flush()
            print(f"{tag}: rc={p.returncode} rows={rows} req={rec['cli_requests']} bytes={rec['cli_bytes']} "
                  f"wall={wall:.1f}s trace={len(trace)}", flush=True)


if __name__ == "__main__":
    main()
