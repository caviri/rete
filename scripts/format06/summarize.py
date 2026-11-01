"""Tables for FORMAT-0x06-DESIGN.md from res/all_sim.json + layouts.
usage: python summarize.py res/all_sim.json files.json > res/tables.md
"best" 0x06 = the variant (exact reads + leaf prediction, or block cache + leaf prediction) with fewer round trips,
ties to fewer bytes: a client that reads exact ranges for lookups and reads ahead on sequential faults.
"""
import json
import statistics
import sys

import hot06

NS = (16, 64, 256)
rows = json.load(open(sys.argv[1]))
files = json.load(open(sys.argv[2]))
kb = lambda n: f"{n / 1024:,.0f}" if n >= 10240 else f"{n / 1024:,.1f}"


def best(r, n):
    a, b = r[f"x06p_{n}k"], r[f"x06pf_{n}k"]
    return min((a, b), key=lambda x: (x[0], x[1]))


def short(f):
    return f.split("/")[-1].replace(".rete", "").replace("opencitations-2026-06-small-full-labels-quad", "oc-small")


# 1. hot sets
print("### Hot set per file (KiB)\n")
print("| file | size MiB | 0x05 hot: dirs+roots / card / token table | 0x05 dead (restart tables) | 0x06 root | card z | schema z | "
      "leaves z (w/o token table) | token table | 0x06 front w/o token table |")
print("|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|")
for served, spec in files.items():
    L = json.load(open(spec["layout"]))
    U = hot06.units(L, spec.get("path"))
    p = hot06.plan(L, U, 16384, spec.get("path"))
    bc = L["by_class"]
    tt = U.get("text", {}).get("z", 0)
    leaves = sum(v["z"] for k, v in p["units"].items() if k != "text")
    front = p["root"] + p["card"] + p["schema"] + leaves
    print(f"| {short(served)} | {L['size'] / 2**20:,.1f} | {kb(bc.get('root', 0) + bc.get('dir', 0) - tt)} / "
          f"{kb(bc.get('card', 0))} / {kb(tt)} | {kb(bc.get('dead', 0))} | {kb(p['root'])} | {kb(p['card'])} | "
          f"{kb(p['schema'])} | {kb(leaves)} | {kb(tt)} | {kb(front)} |")

# 2. OC small
sel = ["open", "card", "demo1-doi-to-work", "c02", "c07", "demo3-journal-issn", "journal_of", "t03-rare-mendelianae",
       "t01-common-study", "c08", "ng-sample", "X2-text-deletions-by-source", "ty-class-hist", "ti-extent"]
print("\n### OpenCitations small (195 MiB, quad index + text + labels): round trips / KiB\n")
print("| query | rows | 0x05 no cache | 0x05 cache | 0x06 N=16K | 0x06 N=64K | 0x06 N=256K | 0x05 hot read / hot set |")
print("|---|--:|--:|--:|--:|--:|--:|--:|")
oc = {r["id"].split(".", 1)[1]: r for r in rows if r["file"].startswith("oc/")}
for q in sel:
    r = oc[q]
    nc, c = r["nocache"], r["cache"]
    print(f"| {q} | {r['rows']} | {nc[0]} / {kb(nc[2])} | {c[0]} / {kb(c[2])} | " +
          " | ".join(f"{best(r, n)[0]} / {kb(best(r, n)[1])}" for n in NS) +
          f" | {kb(r['hot_touched'])} / {kb(r['hot_total'])} |")

# 3. catalog medians per query type
print("\n### Catalog files (5 published 0x05 + 2 rebuilt with labels/schema): median over the 7 files, round trips / KiB\n")
print("| query | 0x05 no cache | 0x05 cache | 0x06 N=16K | 0x06 N=64K | 0x06 N=256K |")
print("|---|--:|--:|--:|--:|--:|")
cat = [r for r in rows if r["file"].split("/")[0] in ("dr", "tci", "sw", "cat", "modern")]
for q in ("open", "card", "iri", "obj-join", "label", "text", "classes"):
    rs = [r for r in cat if r["id"].endswith("." + q)]
    med = lambda xs: statistics.median(xs)
    cells = [f"{med([r['nocache'][0] for r in rs]):.0f} / {kb(med([r['nocache'][2] for r in rs]))}",
             f"{med([r['cache'][0] for r in rs]):.0f} / {kb(med([r['cache'][2] for r in rs]))}"]
    for n in NS:
        cells.append(f"{med([best(r, n)[0] for r in rs]):.0f} / {kb(med([best(r, n)[1] for r in rs]))}")
    print(f"| {q} | " + " | ".join(cells) + " |")

print("\n### Catalog files, every run: 0x05 no cache / cache / 0x06 best at 16K, 64K, 256K (RT / KiB)\n")
print("| file | query | rows | 0x05 no cache | 0x05 cache | 0x06 16K | 0x06 64K | 0x06 256K |")
print("|---|---|--:|--:|--:|--:|--:|--:|")
for r in rows:
    if r["file"].startswith("oc/"):
        continue
    nc, c = r["nocache"], r["cache"]
    print(f"| {short(r['file'])} | {r['id'].split('.', 1)[1]} | {r['rows']} | {nc[0]} / {kb(nc[2])} | {c[0]} / {kb(c[2])} | " +
          " | ".join(f"{best(r, n)[0]} / {kb(best(r, n)[1])}" for n in NS) + " |")

# 4. Meta projection
print("\n### Projected to OpenCitations Meta (59 GiB): the small file's query shapes against Meta's 0x06 front\n")
print("| query | 0x06 N=16K | 0x06 N=64K | 0x06 N=256K |")
print("|---|--:|--:|--:|")
for q in ("open", "card", "demo1-doi-to-work", "c02", "c07", "demo3-journal-issn", "journal_of", "t03-rare-mendelianae",
          "t01-common-study", "ty-class-hist", "ti-extent"):
    m = oc[q]["meta"]
    print(f"| {q} | " + " | ".join(f"{best(m, n)[0]} / {kb(best(m, n)[1])}" for n in NS) + " |")
