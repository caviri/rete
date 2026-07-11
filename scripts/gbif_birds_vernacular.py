#!/usr/bin/env python3
"""Harvest curated European common (vernacular) names for the gbif-birds species,
from the GBIF vernacular-names API. Resumable: appends one JSON line per species
to data/gbif_birds/vernacular.jsonl; re-run to continue after an interruption.

For each of the ~1,104 species we keep the BEST name per European language,
preferring authoritative sources and dropping junk (banding codes, ALL-CAPS,
comma-lists). Each kept name records its GBIF `source` (the checklist it came
from) — the provenance that will be reified onto a VernacularName node at build.
"""
import json, os, re, sys, time, urllib.request

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(HERE, "data", "gbif_birds")
PARTS = os.path.join(DATA, "parts", "part_*.parquet")
OUT = os.path.join(DATA, "vernacular.jsonl")

# Curated European languages (ISO 639-3, as GBIF returns), FR + CA emphasized;
# includes Swiss (roh Romansh) and Iberian regional langs (eus, glg, oci, ast).
EU_LANGS = {
    "eng", "fra", "cat", "spa", "deu", "ita", "por", "nld", "eus", "glg", "oci",
    "ast", "roh", "bre", "cym", "gle", "gla", "glv", "cor", "dan", "swe", "nor",
    "nob", "nno", "fin", "isl", "fao", "pol", "ces", "slk", "hun", "ron", "bul",
    "ell", "hrv", "srp", "bos", "slv", "est", "lav", "lit", "mlt", "sqi", "mkd",
    "bel", "ukr", "rus", "ltz", "fry", "epo",
}
# sources we trust most, in preference order (substring match, case-insensitive)
GOOD_SOURCES = ["ioc", "catalogue of life", "avibase", "eunis", "birdlife",
                "checklist of birds", "clements"]


def clean(name):
    n = (name or "").strip().strip('"').strip()
    if not n or "," in n or ";" in n:          # comma/semicolon lists → skip
        return None
    if n.upper() == n and len(n) <= 6:          # banding codes like BASW
        return None
    if not re.search(r"[A-Za-zÀ-ÿ]{3}", n):     # must have real letters
        return None
    return n


def source_rank(src):
    s = (src or "").lower()
    for i, g in enumerate(GOOD_SOURCES):
        if g in s:
            return i
    return len(GOOD_SOURCES)


def best_per_lang(records):
    """language -> (name, source), choosing the best-sourced clean name."""
    by = {}
    for r in records:
        lg = r.get("language")
        if lg not in EU_LANGS:
            continue
        nm = clean(r.get("vernacularName"))
        if not nm:
            continue
        rank = source_rank(r.get("source"))
        prev = by.get(lg)
        if prev is None or rank < prev[2] or (rank == prev[2] and len(nm) < len(prev[0])):
            by[lg] = (nm, r.get("source") or "", rank)
    return {lg: {"name": v[0], "source": v[1]} for lg, v in by.items()}


def species_list():
    import duckdb
    rows = duckdb.sql(
        f"SELECT DISTINCT specieskey, species FROM read_parquet('{PARTS}') "
        f"WHERE specieskey IS NOT NULL ORDER BY specieskey").fetchall()
    return [(str(k), s) for k, s in rows]


def main():
    done = set()
    if os.path.exists(OUT):
        for line in open(OUT, encoding="utf-8"):
            try:
                done.add(str(json.loads(line)["key"]))
            except Exception:
                pass
    sp = species_list()
    todo = [(k, s) for k, s in sp if k not in done]
    print(f"{len(sp)} species, {len(done)} done, {len(todo)} to fetch", file=sys.stderr, flush=True)
    fh = open(OUT, "a", encoding="utf-8")
    for i, (key, name) in enumerate(todo, 1):
        try:
            url = f"https://api.gbif.org/v1/species/{key}/vernacularNames?limit=400"
            recs = json.load(urllib.request.urlopen(url, timeout=30)).get("results", [])
            names = best_per_lang(recs)
        except Exception as e:
            names = {}
        fh.write(json.dumps({"key": key, "sci": name, "names": names}, ensure_ascii=False) + "\n")
        if i % 100 == 0:
            fh.flush()
            print(f"  {i}/{len(todo)} (last: {name} -> {len(names)} langs)", file=sys.stderr, flush=True)
        time.sleep(0.05)
    fh.flush(); fh.close()
    print("done", file=sys.stderr)


if __name__ == "__main__":
    main()
