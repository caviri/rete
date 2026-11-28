# rete-server benchmark

Measures where a request's time goes, so a change to the server or the engine
can be judged by numbers. Everything runs in containers on synthetic local
graphs; nothing is fetched or uploaded.

```sh
docker build -t rete-server-test clients/server
bash clients/server/bench/run.sh            # about 30 minutes
QUICK=1 bash clients/server/bench/run.sh    # 5 s runs, once each
```

It writes `results/<time>/results.jsonl` and a `report.md`, which it also
prints.

## What it runs

- **Four graphs** of about 200,000 triples each (`make_graphs.py`; `TRIPLES`
  changes the size), built once into the `rete-bench-data` volume.
- **In process** (`engine.py`): each query on `g0` through rete-graph, timed
  step by step: the engine, parsing its JSON, rete-server's reshaping into
  W3C bindings, and writing the answer. This is the floor, with no HTTP.
- **rete-server** under k6 (`load.js`), 1, 4, 16 and 64 clients that each
  send the next request as soon as the last is answered.
- **`rete serve`** (`ghcr.io/caviri/rete-cli`), the same queries on `g0`: an
  all-Rust SPARQL endpoint, as a reference for what HTTP costs without Python.
  It answers one request at a time, so it runs at 1 and 4 clients only.

Both servers get the same four CPUs (`SERVER_CPUS`) and 4 GB; k6 runs on
other CPUs (`LOAD_CPUS`), so the load generator never competes with the
server. Answers are uncompressed except in `large_gzip`.

| Scenario | Request |
|---|---|
| `health` | `GET /health`: the cost of the web stack alone |
| `small` | SPARQL GET, 10 rows |
| `large` | SPARQL GET, 10,000 rows (the row cap) |
| `count` | SPARQL GET, `COUNT(*)` over the whole graph |
| `group` | SPARQL GET, `GROUP BY` class |
| `spread` | `count`, spread over four graphs instead of one |
| `api` | `POST /api/query`, 200 rows, both result shapes |
| `mcp` | MCP `tools/call sparql_query`, the same 200 rows |
| `large_gzip` | `large`, gzip-compressed |

Every run checks one full answer before it starts, so an error page cannot
pass as a fast result.

## Reading the report

- **One client** puts the engine alone, `rete serve` and rete-server side by
  side. The gap between the engine and `rete serve` is HTTP in Rust; the gap
  between `rete serve` and rete-server is what Python adds.
- **CPU ms/req** is the server process's CPU time divided by the requests it
  answered. Python's global lock lets one thread at a time run Python code,
  so this number, not the CPU count, sets rete-server's ceiling.
- `count` against `spread` shows the per-graph lock: on one graph queries take
  turns; on four they can run together.
- **stalled** counts runs where a request was never answered: still open 5 s
  after the run ended. A slow answer that arrives inside those 5 s is not a
  stall.
- The graphs are local, so a query never waits on the network. Against a
  remote file a query also waits for its range reads, which this does not
  show.
