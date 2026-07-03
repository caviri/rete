#!/usr/bin/env python3
"""Join the downloaded IIIF v3 manifests (data/peirce/iiif/*.json, from
scripts/peirce_fetch_iiif.py) back into the Peirce graph as an additional
N-Triples file data/peirce/peirce_iiif.nt:

  component a:iiifManifest <direct mps.lib.harvard.edu manifest URL>
  component a:pageCount    "72"^^xsd:integer
  component a:thumbnail    <first-canvas /full/,250/ jpeg>

Build the final file from both parts:
  rete build /work/data/peirce/peirce.nt /work/data/peirce/peirce_iiif.nt -o ...
"""
import json
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "data", "peirce")
NT = os.path.join(DATA, "peirce.nt")
MDIR = os.path.join(DATA, "iiif")
OUT = os.path.join(DATA, "peirce_iiif.nt")
A = "https://hollisarchives.lib.harvard.edu/ontology#"
INT = "http://www.w3.org/2001/XMLSchema#integer"


def tail(urn):
    t = urn.split("?")[0].rsplit("/", 1)[-1]
    t = re.sub(r"^urn-3:", "", t, flags=re.I)
    return t.replace(":", "_")


def main():
    pairs = re.findall(r"<([^>]+)> <https://hollisarchives\.lib\.harvard\.edu/ontology#digitalContent> <([^>]+)>",
                       open(NT, encoding="utf-8").read())
    n = miss = 0
    with open(OUT, "w", encoding="utf-8", newline="\n") as fh:
        for comp, urn in pairs:
            path = os.path.join(MDIR, tail(urn) + ".json")
            if not os.path.exists(path):
                miss += 1
                continue
            m = json.load(open(path, encoding="utf-8"))
            mid = m.get("id") or m.get("@id")
            canvases = m.get("items", [])
            if mid:
                fh.write(f"<{comp}> <{A}iiifManifest> <{mid}> .\n"); n += 1
            fh.write(f'<{comp}> <{A}pageCount> "{len(canvases)}"^^<{INT}> .\n'); n += 1
            thumb = None
            if canvases:
                th = canvases[0].get("thumbnail") or m.get("thumbnail")
                if th: thumb = th[0].get("id")
                if not thumb:
                    try:
                        body = canvases[0]["items"][0]["items"][0]["body"]
                        svc = body.get("service", [{}])[0]
                        base = svc.get("id") or svc.get("@id")
                        thumb = f"{base}/full/,250/0/default.jpg" if base else None
                    except Exception:
                        pass
            if thumb:
                fh.write(f"<{comp}> <{A}thumbnail> <{thumb}> .\n"); n += 1
    print(f"components with digitalContent: {len(pairs)}, manifests missing: {miss}, triples: {n} -> {OUT}")


if __name__ == "__main__":
    main()
