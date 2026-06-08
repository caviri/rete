#!/usr/bin/env python3
"""Fetch a paper's citations from the OpenCitations index and emit N-Triples,
sharded by the citing paper's year — a real, big, multi-file dataset for the
federation demo.

Model (SPAR/CITO):
  <https://doi.org/CITING> <http://purl.org/spar/cito/cites>     <https://doi.org/CITED> .
  <https://doi.org/CITING> <http://purl.org/dc/terms/date>       "YYYY" .
  <https://doi.org/CITING> rdf:type <http://purl.org/spar/fabio/JournalArticle> .

Usage: python3 scripts/fetch_opencitations.py [DOI] [OUTDIR]
Default DOI: 10.1038/s41586-021-03819-2 (AlphaFold). Default OUTDIR: data/opencitations
"""
import json
import os
import sys
import urllib.request

CITES = "http://purl.org/spar/cito/cites"
DATE = "http://purl.org/dc/terms/date"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
ARTICLE = "http://purl.org/spar/fabio/JournalArticle"

doi = sys.argv[1] if len(sys.argv) > 1 else "10.1038/s41586-021-03819-2"
outdir = sys.argv[2] if len(sys.argv) > 2 else "data/opencitations"
os.makedirs(outdir, exist_ok=True)

url = f"https://api.opencitations.net/index/v1/citations/{doi}"
print(f"fetching {url}", file=sys.stderr)
req = urllib.request.Request(url, headers={"User-Agent": "rete-demo/0.1"})
with urllib.request.urlopen(req, timeout=300) as r:
    data = json.load(r)
print(f"got {len(data)} citation records", file=sys.stderr)


def iri(d):
    return f"<https://doi.org/{d.strip()}>"


def year_of(rec):
    c = (rec.get("creation") or "").strip()
    return c[:4] if len(c) >= 4 and c[:4].isdigit() else "unknown"


# Shard by citing-year. Each shard accumulates triples; also a combined file.
shards = {}
seen_type = set()
combined = open(os.path.join(outdir, "cites-all.nt"), "w", encoding="utf-8")
for rec in data:
    citing = rec.get("citing", "").strip()
    cited = rec.get("cited", "").strip()
    if not citing or not cited:
        continue
    y = year_of(rec)
    lines = [f"{iri(citing)} <{CITES}> {iri(cited)} ."]
    lines.append(f'{iri(citing)} <{DATE}> "{y}" .')
    if citing not in seen_type:
        lines.append(f"{iri(citing)} <{RDF_TYPE}> <{ARTICLE}> .")
        seen_type.add(citing)
    block = "\n".join(lines) + "\n"
    combined.write(block)
    shards.setdefault(y, []).append(block)
combined.close()

for y, blocks in sorted(shards.items()):
    path = os.path.join(outdir, f"cites-{y}.nt")
    with open(path, "w", encoding="utf-8") as f:
        f.writelines(blocks)
    print(f"  shard {y}: {len(blocks)} citing papers -> {path}", file=sys.stderr)

print(f"wrote {len(shards)} year-shards + cites-all.nt to {outdir}", file=sys.stderr)
