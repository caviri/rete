"""Run the landing's questions against a SPARQL server and time them.

Usage: python client.py ENGINE ENDPOINT GRAPH_KEY OUT.json

ENGINE is qlever or oxigraph. Each question runs once right after the
server is up ("first"), then RUNS more times; for QLever the result cache
is cleared before every run, so no run is served from its cache. Oxigraph
has no result cache. Rows are counted from the response: bindings for
SELECT, distinct N-Triples lines for CONSTRUCT (raw line count kept as "lines"). Bytes are the response body size.
Standard library only.
"""
import json
import statistics
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

RUNS = 5
TIMEOUT = 300

engine, endpoint, key, out = sys.argv[1:5]
graph = next(g for g in json.load(open("/bench/questions.json")) if g["key"] == key)


def clear_cache():
    if engine == "qlever":
        urllib.request.urlopen(endpoint + "?cmd=clear-cache", timeout=60).read()


def ask(q):
    construct = "CONSTRUCT" in q.upper().split("WHERE")[0]
    accept = "application/n-triples" if construct else "application/sparql-results+json"
    req = urllib.request.Request(
        endpoint,
        data=urllib.parse.urlencode({"query": q}).encode(),
        headers={"Accept": accept, "Content-Type": "application/x-www-form-urlencoded"},
    )
    t = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            body = r.read()
            ctype = r.headers.get("Content-Type", "")
    except urllib.error.HTTPError as e:
        return {"error": f"HTTP {e.code}: {e.read()[:300].decode('utf-8', 'replace')}"}
    except Exception as e:  # timeout, reset
        return {"error": repr(e)[:300]}
    ms = (time.perf_counter() - t) * 1000
    if construct:
        lines = [line for line in body.decode("utf-8").splitlines() if line.strip() and not line.startswith("#")]
        rows = len(set(lines))  # a CONSTRUCT result is a set of triples; QLever can repeat one
        return {"ms": (time.perf_counter() - t) * 1000, "rows": rows, "lines": len(lines), "bytes": len(body), "type": ctype}
    else:
        rows = len(json.loads(body)["results"]["bindings"])
    return {"ms": ms, "rows": rows, "bytes": len(body), "type": ctype}


results = []
for qu in graph["questions"]:
    clear_cache()
    first = ask(qu["q"])
    runs = []
    if "error" not in first:
        for _ in range(RUNS):
            clear_cache()
            r = ask(qu["q"])
            if "error" in r:
                first = r
                break
            runs.append(r["ms"])
    row = {"label": qu["label"], "first": first}
    if runs:
        row["median_ms"] = statistics.median(runs)
        row["runs_ms"] = runs
    results.append(row)
    print(engine, key, qu["label"], json.dumps({k: v for k, v in row.items() if k != "runs_ms"}), flush=True)

json.dump(results, open(out, "w"), indent=1)
