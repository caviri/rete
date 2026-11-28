"""results.jsonl (stdin) → a Markdown report (stdout). Medians over repetitions."""
import json
import statistics
import sys

rows = [json.loads(line) for line in sys.stdin if line.strip()]
meta = next((r for r in rows if r["kind"] == "meta"), {})
engine = {r["query"]: r for r in rows if r["kind"] == "engine"}
load = [r for r in rows if r["kind"] == "load"]


def seconds(duration):
    return float(duration[:-1]) if duration.endswith("s") else None


RUN_S = seconds(meta.get("duration", ""))
GRACE_S = 5  # gracefulStop in load.js


def cell(server, scenario, vus):
    reps = [r for r in load if (r["server"], r["scenario"], r["vus"]) == (server, scenario, vus)]
    if not reps:
        return None
    med = lambda f: statistics.median(f(r) for r in reps)
    return {
        # Requests answered per second of the run itself. k6's own rate also
        # divides by the time a stalled request kept the run open.
        "rps": med(lambda r: r["k6"]["reqs"] / RUN_S if RUN_S else r["k6"]["rps"]),
        # A run that used all of load.js's 5 s grace had a request that was
        # never answered; a slow tail that finishes inside the grace is not.
        "stalled": sum(r["k6"].get("run_ms", 0) >= ((RUN_S or 1e9) + GRACE_S) * 1000 - 100 for r in reps),
        "runs": len(reps),
        "p50": med(lambda r: r["k6"]["p50"]),
        "p95": med(lambda r: r["k6"]["p95"]),
        "p99": med(lambda r: r["k6"]["p99"]),
        "failed": max(r["k6"]["failed"] for r in reps),
        "cpu": med(lambda r: r["cpu_s"] * 1000 / max(r["k6"]["reqs"], 1)),
    }


def ms(v):
    return "–" if v is None else (f"{v:.2f}" if v < 10 else f"{v:.0f}")


print(f"# rete-server benchmark, {meta.get('date', '?')}\n")
print(f"{meta.get('server')} against {meta.get('rust')} (`rete serve`). Synthetic graphs of "
      f"~{meta.get('triples')} triples, local files, warm. Each server on CPUs "
      f"{meta.get('server_cpus')}, the k6 load generator on {meta.get('load_cpus')}. "
      f"{meta.get('duration')} per run, median of {meta.get('reps')}.\n")

if engine:
    print("## In process\n")
    print("One query in Python, step by step, the way `/sparql` runs it (ms, median).\n")
    print("| query | rows | engine | parse | shape | dump | total | envelope KB | response KB |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for name, e in engine.items():
        print(f"| {name} | {e['rows']} | {ms(e['engine_ms'])} | {ms(e['parse_ms'])} | "
              f"{ms(e['shape_ms'])} | {ms(e['dump_ms'])} | {ms(e['total_ms'])} | "
              f"{e['envelope_kb']} | {e['response_kb']} |")
    print()

print("## One client\n")
print("Median latency (ms) of one request at a time: the engine alone, `rete serve`, "
      "rete-server.\n")
print("| query | engine alone | in process total | rete serve | rete-server | rete-server overhead |")
print("|---|---:|---:|---:|---:|---:|")
for name in ("small", "large", "count", "group"):
    e = engine.get(name, {})
    rust, py = cell("rust", name, 1), cell("py", name, 1)
    over = py["p50"] - e["engine_ms"] if py and e else None
    print(f"| {name} | {ms(e.get('engine_ms'))} | {ms(e.get('total_ms'))} | "
          f"{ms(rust and rust['p50'])} | {ms(py and py['p50'])} | {ms(over)} |")
print()

print("## Under load\n")
print("req/s, latency percentiles (ms), failed share, server CPU per request (ms), and "
      "runs where a request was never answered (still open 5 s after the run).\n")
print("| scenario | clients | server | req/s | p50 | p95 | p99 | failed | CPU ms/req | stalled |")
print("|---|---:|---|---:|---:|---:|---:|---:|---:|---:|")
for scenario in dict.fromkeys(r["scenario"] for r in load):
    for vus in sorted({r["vus"] for r in load if r["scenario"] == scenario}):
        for server, label in (("py", "rete-server"), ("rust", "rete serve")):
            c = cell(server, scenario, vus)
            if c:
                print(f"| {scenario} | {vus} | {label} | {c['rps']:.0f} | {ms(c['p50'])} | "
                      f"{ms(c['p95'])} | {ms(c['p99'])} | {c['failed']:.1%} | {ms(c['cpu'])} | "
                      f"{c['stalled']}/{c['runs']} |")
print()

print("## Memory\n")
for server, label in (("py", "rete-server"), ("rust", "rete serve")):
    peaks = [r["hwm_mb"] for r in load if r["server"] == server]
    if peaks:
        print(f"- {label}: peak RSS {max(peaks)} MiB")
