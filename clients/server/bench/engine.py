"""One query in process, step by step, the way rete-server's /sparql runs it.

  engine  rete-graph's query(): evaluation in Rust and its JSON envelope
  parse   json.loads of that envelope (rete-graph's query_raw does this)
  shape   rete-server turning the envelope's rows into W3C bindings
  dump    orjson writing the SPARQL-JSON body

Prints one JSON line per query: the median of RUNS runs after a warm-up.
"""
import json
import os
import statistics
import time

import orjson
import rete_graph as rete

from rete_server.engine import _bindings

QUERIES = json.loads(os.environ["QUERIES"])
RUNS = int(os.environ.get("RUNS", "20"))

graph = rete.open("/data/g0.rete")
for name, query in QUERIES.items():
    for _ in range(3):
        graph.query_raw(query)
    steps = {"engine": [], "parse": [], "shape": [], "dump": []}
    for _ in range(RUNS):
        t0 = time.perf_counter()
        raw = graph._g.query(query, reason=False)
        t1 = time.perf_counter()
        env = json.loads(raw)
        t2 = time.perf_counter()
        variables, bindings, _, _ = _bindings(env, 10_000, "w3c")
        t3 = time.perf_counter()
        body = orjson.dumps({"head": {"vars": variables}, "results": {"bindings": bindings}})
        t4 = time.perf_counter()
        for key, value in zip(steps, (t1 - t0, t2 - t1, t3 - t2, t4 - t3)):
            steps[key].append(value * 1000)
    out = {f"{k}_ms": round(statistics.median(v), 3) for k, v in steps.items()}
    out["total_ms"] = round(sum(out.values()), 3)
    print(json.dumps({"kind": "engine", "query": name, "rows": len(env.get("rows") or []),
                      "envelope_kb": round(len(raw) / 1024, 1),
                      "response_kb": round(len(body) / 1024, 1), **out}), flush=True)
