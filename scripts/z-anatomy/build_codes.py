"""Extract clinical crosswalk codes (ICD-10/9/11, ICD-O, SNOMED CT, MeSH, OMIM,
UMLS) from the Disease Ontology xrefs, keyed by DOID id, for build_nt.py to
attach to the disease terms it links to anatomy. Writes derived/codes.json.
"""
import json, os, re

DO = "data/disease-ontology/raw/HumanDiseaseOntology-2026-06-30/HumanDiseaseOntology-2026-06-30/src/ontology/doid.obo"
OUT = "data/z-anatomy/derived/codes.json"

# xref namespace -> our short key (only these are kept)
NS = {"ICD10CM": "icd10", "ICD9CM": "icd9", "ICD11": "icd11", "ICDO": "icdo",
      "SNOMEDCT_US_2023_03_01": "snomed", "SNOMEDCT": "snomed",
      "MESH": "mesh", "OMIM": "omim", "UMLS_CUI": "umls", "UMLS": "umls",
      "NCI": "nci"}

codes = {}
cur = None
for line in open(DO, encoding="utf-8", errors="replace"):
    line = line.rstrip("\n")
    if line == "[Term]":
        cur = None
    elif line.startswith("id: DOID:"):
        cur = line[4:].strip()
    elif line.startswith("xref: ") and cur:
        m = re.match(r"xref:\s+([A-Za-z0-9_]+):(\S+)", line)
        if not m:
            continue
        ns, code = m.group(1), m.group(2)
        key = NS.get(ns)
        if key:
            codes.setdefault(cur, {}).setdefault(key, [])
            if code not in codes[cur][key]:
                codes[cur][key].append(code)

os.makedirs(os.path.dirname(OUT), exist_ok=True)
json.dump(codes, open(OUT, "w", encoding="utf-8"), ensure_ascii=False)
from collections import Counter
c = Counter(k for v in codes.values() for k in v)
print(f"DOID terms with codes: {len(codes)} -> {OUT}")
print("by system:", dict(c))
# sample
for did in ("DOID:5082", "DOID:1936", "DOID:9351"):
    if did in codes:
        print(f"  {did}: {codes[did]}")
