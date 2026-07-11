#!/usr/bin/env python3
"""Fetch IUCN Red List conservation status for the GBIF bird species from Wikidata.

Input : data/gbif_birds/vernacular.jsonl  (has {key, sci, names})  -- we reuse `sci`.
Output: data/gbif_birds/iucn.jsonl        {key, sci, status, statusLabel, wd}

Wikidata match is on the taxon name (wdt:P225) and the IUCN conservation status
(wdt:P141). The status value is a Wikidata item; we map it to the standard
2-letter Red List category by its English label. Keyless, batched with VALUES.
"""
import json, os, sys, time, urllib.parse, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "..", "data", "gbif_birds")
IN = os.path.join(DATA, "vernacular.jsonl")
OUT = os.path.join(DATA, "iucn.jsonl")
ENDPOINT = "https://query.wikidata.org/sparql"
UA = "rete-graphplaza/1.0 (https://graphplaza.com; dataset enrichment) python-urllib"

# Red List category from the English label of the P141 value. Wikidata labels
# vary ("Endangered status", "Critically Endangered", "vulnerable species", …),
# so classify by ordered keyword match — MOST specific first, since "critically
# endangered" contains "endangered" and "extinct in the wild" contains "extinct".
CODE_RULES = [
    ("critically endangered", "CR"),
    ("extinct in the wild", "EW"),
    ("near threatened", "NT"),
    ("least concern", "LC"),
    ("data deficient", "DD"),
    ("not evaluated", "NE"),
    ("endangered", "EN"),
    ("vulnerable", "VU"),
    ("extinct", "EX"),
]
# Categories we treat as "sensitive" (threatened or worse).
SENSITIVE = {"NT", "VU", "EN", "CR", "EW", "EX"}


def code_for(label):
    s = (label or "").strip().lower()
    for kw, code in CODE_RULES:
        if kw in s:
            return code
    return None


def sparql(query, tries=5):
    url = ENDPOINT + "?" + urllib.parse.urlencode({"query": query, "format": "json"})
    for i in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/sparql-results+json"})
            with urllib.request.urlopen(req, timeout=90) as r:
                return json.loads(r.read().decode("utf-8"))["results"]["bindings"]
        except Exception as e:
            wait = 3 * (i + 1)
            print(f"  retry {i+1}/{tries} after {wait}s: {e}", file=sys.stderr)
            time.sleep(wait)
    raise RuntimeError("SPARQL failed after retries")


def main():
    species = []
    seen = set()
    for line in open(IN, encoding="utf-8"):
        if not line.strip():
            continue
        r = json.loads(line)
        sci = (r.get("sci") or "").strip()
        # keep only proper binomials (skip hybrids "A x B" and blanks)
        if not sci or " x " in f" {sci} " or sci.count(" ") == 0:
            continue
        if sci in seen:
            continue
        seen.add(sci)
        species.append((r["key"], sci))
    print(f"{len(species)} species to resolve", file=sys.stderr)

    out = open(OUT, "w", encoding="utf-8")
    n_found = 0
    B = 120
    for i in range(0, len(species), B):
        batch = species[i:i + B]
        values = " ".join('"%s"' % s.replace('"', '\\"') for _, s in batch)
        q = f"""
SELECT ?name ?status ?statusLabel ?taxon WHERE {{
  VALUES ?name {{ {values} }}
  ?taxon wdt:P225 ?name ; wdt:P141 ?status .
  SERVICE wikibase:label {{ bd:serviceParam wikibase:language "en". }}
}}"""
        rows = sparql(q)
        by_name = {}
        for b in rows:
            nm = b["name"]["value"]
            lbl = b.get("statusLabel", {}).get("value", "")
            code = code_for(lbl)
            if code and nm not in by_name:  # first hit per name
                by_name[nm] = (code, lbl, b["status"]["value"], b["taxon"]["value"])
        for key, sci in batch:
            if sci in by_name:
                code, lbl, st, tax = by_name[sci]
                out.write(json.dumps({
                    "key": key, "sci": sci, "status": code, "statusLabel": lbl,
                    "sensitive": code in SENSITIVE, "wd": tax,
                }, ensure_ascii=False) + "\n")
                n_found += 1
        print(f"  batch {i//B+1}: +{len(by_name)} (total {n_found})", file=sys.stderr)
        time.sleep(1.0)
    out.close()
    print(f"iucn: {n_found} species with a Red List status -> {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
