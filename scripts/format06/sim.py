"""Simulate a 0x06 open + query from a measured 0x05 trace (no block cache), and tabulate it next to the measured
0x05 runs (without and with the block cache).

Model (see FORMAT-0x06-DESIGN.md, "Expected gain"):
- read 1 fetches bytes [0, N): header, root block, card, then leaf directory blocks in LEAF_ORDER, as far as N reaches;
  a root larger than N costs one more read (the rest of the root);
- every 0x05 read that STARTS in a hot region (header, framing, directory, card, schema) is served by the 0x06 front;
- every 0x05 read that starts in a payload region stays, in its 0x05 wave (round trip), with its 0x05 bytes;
- a payload read needs the leaf block of its directory unit (dict section, permutation, quad order + graph directory,
  label nodes, token table): if that block lies past N and was not fetched yet, the leaf blocks a wave needs are
  fetched together in one extra round trip just before it.

usage: python sim.py AN.json[,AN2.json] --files files.json --out table.json [--B 16384]
files.json maps a served path to {"layout": "layouts/x.json", "path": "local file"}.
"""
import bisect
import json
import sys
from collections import defaultdict

import hot06

NS = (16 << 10, 64 << 10, 256 << 10)


def region_at(regs, starts, x):
    i = bisect.bisect_right(starts, x) - 1
    if i < 0:
        return None
    r = regs[i]
    return r if r[0] <= x < r[1] else None


def simulate(rep, L, regs, starts, plan, N, blk=None, prefetch=False, size06=None):
    size = size06 or L["size"]
    if size <= N:  # the whole file came with the first read
        return 1, size, 0, 0
    rt, nbytes = 1, min(N, size)
    if plan["root"] > N:
        rt += 1
        nbytes += plan["root"] - N
    fetched = set()
    waves = defaultdict(list)
    front = {}
    for q in rep["reads"]:
        if q["a"] is None:
            continue
        r = region_at(regs, starts, q["a"])
        if r is None:
            continue
        s, e, sec, part, cls = r
        if cls == "cold":
            waves[q["wave"]].append((q, r))
        elif sec in ("card", "schema") and sec not in front:
            front[sec] = q["wave"]
    order = sorted(set(waves) | set(front.values()))
    leaf_bytes = 0
    cold_bytes = 0
    blocks = set()
    seen_cold = False
    for w in order:
        need = {}
        for item, fw in front.items():
            if fw == w and plan[f"{item}_off"] + plan[item] > N and item not in fetched:
                need[item] = plan[item]
        for q, (s, e, sec, part, cls) in waves.get(w, []):
            u = hot06.unit_of(sec, part)
            us = [u] if u else []
            if u and u.startswith("quad.G+"):
                us.append("quad.graphs")
            for uu in us:
                P = plan["units"].get(uu)
                if not P:
                    continue
                lo, hi = plan["data"][uu]
                frac = (q["a"] - lo) / max(1, hi - lo)
                idx = min(P["leaves"] - 1, max(0, int(frac * P["leaves"])))
                b0 = P["start"] + idx * P["leaf"]
                if b0 + P["leaf"] <= N or (uu, idx) in fetched:
                    continue
                need[(uu, idx)] = P["leaf"]
        if need:
            # with leaf prediction, a leaf a later wave needs rides with the previous payload wave
            if not (prefetch and seen_cold):
                rt += 1
            leaf_bytes += sum(need.values())
            fetched |= set(need)
        if waves.get(w):
            seen_cold = True
        if waves.get(w) and blk:
            miss = set()
            for q, _r in waves[w]:
                for k in range(q["a"] // blk, (q["b"] - 1) // blk + 1):
                    if k not in blocks:
                        miss.add(k)
            if miss:
                rt += 1
                cold_bytes += len(miss) * blk
                blocks |= miss
        elif waves.get(w):
            rt += 1
            cold_bytes += sum(q["len"] for q, _ in waves[w])
    return rt, int(nbytes + leaf_bytes + cold_bytes), int(leaf_bytes), int(cold_bytes)


def measured(rep):
    reqs = [q for q in rep["reads"]]
    waves = max((q["wave"] for q in reqs), default=0)
    return waves, len(reqs), sum(q["len"] for q in reqs)


def hot_touched(rep, regs, starts):
    """Distinct hot bytes (root/dir/card/schema regions) the 0x05 run read, clipped to those regions."""
    iv = []
    for q in rep["reads"]:
        if q["a"] is None:
            continue
        i = max(0, bisect.bisect_right(starts, q["a"]) - 1)
        while i < len(regs) and regs[i][0] < q["b"]:
            s, e, sec, part, cls = regs[i]
            if cls in ("root", "dir", "card"):
                lo, hi = max(s, q["a"]), min(e, q["b"])
                if hi > lo:
                    iv.append((lo, hi))
            i += 1
    tot, end = 0, -1
    for lo, hi in sorted(iv):
        if hi <= end:
            continue
        tot += hi - max(lo, end)
        end = hi
    return tot


VARIANTS = (("x06", {}), ("x06p", {"prefetch": True}), ("x06bc", {"blk": True}), ("x06pf", {"blk": True, "prefetch": True}))


def run_variants(row, nc, L, regs, starts, plan, size06):
    blk = (128 << 10) if L["size"] > (100 << 20) else (64 << 10)
    for N in NS:
        for name, kw in VARIANTS:
            row[f"{name}_{N >> 10}k"] = simulate(nc, L, regs, starts, plan, N, blk if kw.get("blk") else None,
                                                 kw.get("prefetch", False), size06)


def main():
    reps = []
    for f in sys.argv[1].split(","):
        reps += json.load(open(f))
    files = json.load(open(sys.argv[sys.argv.index("--files") + 1]))
    B = int(sys.argv[sys.argv.index("--B") + 1]) if "--B" in sys.argv else 16384
    cache = {}
    for served, spec in files.items():
        L = json.load(open(spec["layout"]))
        regs = sorted(tuple(r) for r in L["regions"])
        U = hot06.units(L, spec.get("path"))
        plan = hot06.plan(L, U, B, spec.get("path"))
        plan["data"] = {u: tuple(U[u]["data"]) if U[u]["data"][0] is not None else (0, 1) for u in U}
        hot_total = sum(e - s for s, e, sec, part, cls in regs if cls in ("root", "dir", "card"))
        dead = sum(e - s for s, e, sec, part, cls in regs if cls == "dead")
        cache[served] = (L, regs, [r[0] for r in regs], plan, hot_total, L["size"] - dead)
    proj = None
    if "--project" in sys.argv:
        # --project SERVED=LAYOUT.json,RATIOS.json : replay SERVED's traces against that layout's 0x06 front
        served, rest = sys.argv[sys.argv.index("--project") + 1].split("=", 1)
        lp, rp = rest.split(",")
        PL = json.load(open(lp))
        R = json.load(open(rp))
        ratios = {"dict": R["dict"], "index": R["index"]}
        PU = hot06.units(PL, None, ratios)
        pplan = hot06.plan(PL, PU, B, None, card=int(R["card"]), schema=int(R["schema"]))
        proj = (served, PL, pplan)
    by = defaultdict(dict)
    for rep in reps:
        by[(rep["file"], rep["id"])][rep["mode"]] = rep
    rows = []
    for (served, qid), modes in by.items():
        if served not in cache or "nocache" not in modes:
            continue
        L, regs, starts, plan, hot_total, size06 = cache[served]
        nc = modes["nocache"]
        row = {"file": served, "id": qid, "rows": nc["rows"], "nocache": measured(nc),
               "cache": measured(modes["cache"]) if "cache" in modes else None,
               "hot_total": hot_total, "hot_touched": hot_touched(nc, regs, starts),
               "root": plan["root"], "card": plan["card"], "hot_end": plan["hot_end"]}
        run_variants(row, nc, L, regs, starts, plan, size06)
        if proj and served == proj[0]:
            p = dict(proj[2])
            p["data"] = plan["data"]  # the trace's own data regions; leaf indices scale to the projected leaves
            prow = {"projected": True, **{k: row[k] for k in ("file", "id", "rows")}, "root": p["root"],
                    "card": p["card"], "hot_end": p["hot_end"]}
            run_variants(prow, nc, L, regs, starts, p, proj[1]["size"])
            row["meta"] = prow
        rows.append(row)
    json.dump(rows, open(sys.argv[sys.argv.index("--out") + 1], "w"), indent=0)
    kb = lambda n: f"{n / 1024:,.0f}" if n >= 10240 else f"{n / 1024:,.1f}"
    cell = lambda r, k: f"{r[k][0]} / {kb(r[k][1])}"
    print("| file | query | rows | 0x05 no cache: RT / req / KiB | 0x05 cache: RT / req / KiB | "
          "0x06 exact 16K / 64K / 256K: RT / KiB | 0x06 exact+predict | 0x06 cache | 0x06 cache+predict | hot touched / total KiB |")
    print("|---|---|--:|--:|--:|--:|--:|--:|--:|--:|")
    for r in rows:
        nc, c = r["nocache"], r["cache"] or (0, 0, 0)
        groups = [", ".join(cell(r, f"{v}_{n >> 10}k") for n in NS) for v, _ in VARIANTS]
        print(f"| {r['file'].split('/')[-1].replace('.rete', '')} | {r['id'].split('.', 1)[1]} | {r['rows']} | "
              f"{nc[0]} / {nc[1]} / {kb(nc[2])} | {c[0]} / {c[1]} / {kb(c[2])} | " + " | ".join(groups) +
              f" | {kb(r['hot_touched'])} / {kb(r['hot_total'])} |")
    if proj:
        print()
        print(f"Projected to {proj[1]['source']}: root {proj[2]['root']:,} B, card {proj[2]['card']:,} B, "
              f"schema {proj[2]['schema']:,} B, hot end {proj[2]['hot_end']:,} B; leaves: " +
              ", ".join(f"{u} {v['leaves']}x{v['B'] >> 10}K" for u, v in proj[2]["units"].items()))
        print("| query | 0x06 exact 16K / 64K / 256K | 0x06 exact+predict | 0x06 cache | 0x06 cache+predict |")
        print("|---|--:|--:|--:|--:|")
        for r in rows:
            if "meta" in r:
                m = r["meta"]
                groups = [", ".join(cell(m, f"{v}_{n >> 10}k") for n in NS) for v, _ in VARIANTS]
                print(f"| {r['id'].split('.', 1)[1]} | " + " | ".join(groups) + " |")


if __name__ == "__main__":
    main()
