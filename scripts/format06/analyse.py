"""Join the proxy log with the layout maps: per run, every range read with its wave (round trip), offset, length,
and the bytes it touched in each class/section/part; then simulate a 0x06 open at first-read sizes N.

usage: python analyse.py PROXY.log RESULTS.jsonl[,RESULTS2...] LAYOUTMAP.json (one per served file: path=json)
       --rtt MS --out OUT.json [--detail TAG]
Layout maps are given as  file_path_under_nginx=layouts/x.json  pairs.
"""
import bisect
import json
import sys
from collections import defaultdict

HOT = ("root", "dir", "card")


def load_layout(path):
    L = json.load(open(path))
    regs = sorted(tuple(r) for r in L["regions"])
    return L, regs, [r[0] for r in regs]


def overlap(regs, starts, a, b):
    """Bytes of [a,b) in each (section, part, class)."""
    out = defaultdict(int)
    i = max(0, bisect.bisect_right(starts, a) - 1)
    while i < len(regs) and regs[i][0] < b:
        s, e, sec, part, cls = regs[i]
        lo, hi = max(a, s), min(b, e)
        if hi > lo:
            out[(sec, part, cls)] += hi - lo
        i += 1
    return out


def parse_log(log, rtt):
    runs, cur, reqs = {}, None, []
    for line in open(log):
        t, method, path, rng = (line.rstrip("\n").split("\t") + [""] * 4)[:4]
        t = float(t)
        if path.startswith("/__mark__/"):
            tag = path[len("/__mark__/"):]
            if tag.endswith("/start"):
                cur, reqs = tag[:-6], []
            elif tag.endswith("/end") and cur == tag[:-4]:
                runs[cur] = reqs
                cur = None
            continue
        if cur is not None:
            reqs.append((t, method, path, rng))
    out = {}
    for tag, reqs in runs.items():
        waves, start, recs = 0, None, []
        for t, method, path, rng in reqs:
            if start is None or t - start > rtt / 2:
                waves, start = waves + 1, t
            a = b = None
            if rng.startswith("bytes="):
                x, y = rng[6:].split("-")
                a, b = int(x), int(y) + 1
            recs.append({"wave": waves, "t": t, "method": method, "path": path.lstrip("/"), "a": a, "b": b})
        out[tag] = recs
    return out


def main():
    log, results = sys.argv[1], sys.argv[2].split(",")
    rtt = float(sys.argv[sys.argv.index("--rtt") + 1]) / 1000
    maps = {}
    for arg in sys.argv[3:]:
        if "=" in arg and not arg.startswith("--"):
            f, p = arg.split("=", 1)
            maps[f] = load_layout(p)
    runs = parse_log(log, rtt)
    recs = []
    for rf in results:
        for line in open(rf):
            recs.append(json.loads(line))
    report = []
    for r in recs:
        tag = f"{r['mode']}/{r['id']}/{r['run']}"
        reqs = runs.get(tag)
        if reqs is None:
            continue
        if r["file"] not in maps:
            continue
        L, regs, starts = maps[r["file"]]
        rows = []
        for q in reqs:
            if q["a"] is None:
                rows.append({**q, "len": 0, "cls": {}, "parts": {}})
                continue
            ov = overlap(regs, starts, q["a"], q["b"])
            cls = defaultdict(int)
            for (_s, _p, c), n in ov.items():
                cls[c] += n
            rows.append({**q, "len": q["b"] - q["a"], "cls": dict(cls),
                         "parts": {f"{s}:{p}": n for (s, p, c), n in ov.items()}})
        report.append({"tag": tag, "mode": r["mode"], "id": r["id"], "file": r["file"], "rows": r["rows"],
                       "rc": r["rc"], "wall": r["wall"], "cli_requests": r["cli_requests"],
                       "cli_bytes": r["cli_bytes"], "reads": rows, "size": L["size"]})
    json.dump(report, open(sys.argv[sys.argv.index("--out") + 1], "w"))
    if "--detail" in sys.argv:
        want = sys.argv[sys.argv.index("--detail") + 1]
        for rep in report:
            if rep["tag"] == want:
                for q in rep["reads"]:
                    parts = ", ".join(f"{k}={v}" for k, v in sorted(q["parts"].items(), key=lambda kv: -kv[1])[:4])
                    print(f"w{q['wave']:>2} {q['method']} {q['a']}+{q['len']}  {parts}")


if __name__ == "__main__":
    main()
