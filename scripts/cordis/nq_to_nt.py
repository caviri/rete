"""Convert the CORDIS .nq files to N-Triples (drop the named-graph term) so all
data lands in the default graph — plain SPARQL and the type pyramid then see the
real classes (Project/Result/Grant/…), not just the ontology.

The named graph in this dump is only CORDIS's per-entity export partition, with
no RDF semantics, so collapsing it into one default graph is the right call.

Output: data/cordis/nt/<Entity>.nt
"""

import os
from concurrent.futures import ProcessPoolExecutor

import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from nq_to_triples_parquet import parse_line  # reuse the explicit parser

NQ = r"D:\pro\rete\data\cordis\nq"
OUT = r"D:\pro\rete\data\cordis\nt"

_ESC = str.maketrans({"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"})


def term_obj(obj, otype, lang, dt):
    if otype == "iri":
        return f"<{obj}>"
    if otype == "bnode":
        return obj
    lit = '"' + obj.translate(_ESC) + '"'
    if dt:
        return lit + f"^^<{dt}>"
    if lang:
        return lit + f"@{lang}"
    return lit


def term_sp(v):
    return v if v.startswith("_:") else f"<{v}>"


def convert(member):
    os.makedirs(OUT, exist_ok=True)
    src = os.path.join(NQ, member)
    dst = os.path.join(OUT, member[:-3] + ".nt")
    n = 0
    with open(src, "r", encoding="utf-8", errors="replace") as f, \
         open(dst, "w", encoding="utf-8", newline="\n") as out:
        buf = []
        for line in f:
            r = parse_line(line)
            if r is None:
                continue
            s, p, o, otype, lang, dt, _g = r
            buf.append(f"{term_sp(s)} <{p}> {term_obj(o, otype, lang, dt)} .\n")
            n += 1
            if len(buf) >= 200000:
                out.write("".join(buf))
                buf.clear()
        out.write("".join(buf))
    return member, n


def main():
    os.makedirs(OUT, exist_ok=True)
    members = [f for f in os.listdir(NQ) if f.endswith(".nq")]
    with ProcessPoolExecutor(max_workers=6) as pool:
        for member, n in pool.map(convert, members):
            print(f"{member[:-3]+'.nt':24s} {n:>12,} triples", flush=True)


if __name__ == "__main__":
    main()
