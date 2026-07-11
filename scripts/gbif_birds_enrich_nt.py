#!/usr/bin/env python3
"""Species-level enrichment for the GBIF birds .rete, emitted as N-Triples-STAR.

Adds, per species taxon (IRI = .../taxon/species/<specieskey>):
  * dwc:vernacularName  — common name in up to 47 European languages (lang-tagged)
  * v:iucnStatus        — IUCN Red List category (LC/NT/VU/EN/CR/EW/EX)
  * v:sensitive         — true for threatened species (NT and worse)
  * v:recordCount / v:estimatedIndividuals / v:firstYear / v:lastYear
                        — occurrence statistics over the ES+CH extract

Every ADDED statement carries **RDF-star provenance** — a quoted triple annotated
with where it came from and when we added it:

    << <sp> dwc:vernacularName "Hirondelle rustique"@fr >>
        dct:source "Catalogue of Life" ;
        prov:generatedAtTime "2026-07-11"^^xsd:date ;
        prov:wasDerivedFrom <https://api.gbif.org/v1/species/KEY/vernacularNames> .

This is the point of RDF-star here: the annotations attach to a *statement* (a
name assertion has no node of its own), so provenance lives on the quoted triple.
Occurrence-level provenance (basisOfRecord, dataset, individualCount) stays plain,
because an occurrence already IS a node.

Inputs : data/gbif_birds/vernacular.jsonl, iucn.jsonl, parts/*.parquet
Output : data/gbif_birds/birds_enrich.nt   (N-Triples-star)
"""
import glob
import json
import os
import sys

import duckdb

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "..", "data", "gbif_birds")
VERN = os.path.join(DATA, "vernacular.jsonl")
IUCN = os.path.join(DATA, "iucn.jsonl")
PARTS = os.path.join(DATA, "parts", "part_*.parquet")
OUT = os.environ.get("GBIF_OUT_ENRICH", os.path.join(DATA, "birds_enrich.nt"))
ADDED = os.environ.get("GBIF_ENRICH_DATE", "2026-07-11")   # "when they were added"

NS = "https://rete.graphplaza.com/gbif/"
TAXON = NS + "taxon/"
V = NS + "vocab#"
DWC = "http://rs.tdwg.org/dwc/terms/"
DCT = "http://purl.org/dc/terms/"
PROV = "http://www.w3.org/ns/prov#"
XSD = "http://www.w3.org/2001/XMLSchema#"

VERN_NAME = DWC + "vernacularName"
IUCN_STATUS = V + "iucnStatus"
SENSITIVE_P = V + "sensitive"
REC_COUNT = V + "recordCount"
EST_INDIV = V + "estimatedIndividuals"
FIRST_YEAR = V + "firstYear"
LAST_YEAR = V + "lastYear"
SOURCE = DCT + "source"
GENERATED = PROV + "generatedAtTime"
DERIVED = PROV + "wasDerivedFrom"

# ISO 639-3 (as GBIF returns) -> BCP-47 primary subtag. All 47 harvested codes
# have a 2-letter equivalent; anything unmapped falls back to the 3-letter code.
LANG = {
    "eng": "en", "deu": "de", "fra": "fr", "spa": "es", "nld": "nl", "por": "pt",
    "swe": "sv", "ita": "it", "pol": "pl", "fin": "fi", "nob": "nb", "lit": "lt",
    "slk": "sk", "ces": "cs", "hun": "hu", "cat": "ca", "epo": "eo", "est": "et",
    "srp": "sr", "cym": "cy", "hrv": "hr", "nor": "no", "lav": "lv", "slv": "sl",
    "isl": "is", "dan": "da", "gle": "ga", "bre": "br", "nno": "nn", "ron": "ro",
    "glg": "gl", "fao": "fo", "eus": "eu", "glv": "gv", "gla": "gd", "ltz": "lb",
    "sqi": "sq", "cor": "kw", "roh": "rm", "fry": "fy", "mlt": "mt", "oci": "oc",
    "rus": "ru", "bos": "bs", "bul": "bg", "ell": "el", "mkd": "mk",
}


def esc(s):
    """Escape a string literal for N-Triples."""
    return (s.replace("\\", "\\\\").replace('"', '\\"')
            .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def sp_iri(key):
    return f"{TAXON}species/{key}"


class Writer:
    def __init__(self, path):
        self.fh = open(path, "w", encoding="utf-8")
        self.n = 0

    def triple(self, s, p, o):
        """s, p already IRIs (no <>); o is a full object term (with quotes/<> )."""
        self.fh.write(f"<{s}> <{p}> {o} .\n")
        self.n += 1

    def annotate(self, s, p, o, provs):
        """Emit the base statement PLUS RDF-star provenance annotations on it.

        `provs` is a list of (pred, object-term). The quoted triple is written
        tight (`<<...>>`) — rete re-canonicalizes, but we match its form exactly.
        """
        self.triple(s, p, o)
        qt = f"<<<{s}> <{p}> {o}>>"
        for pp, oo in provs:
            self.fh.write(f"{qt} <{pp}> {oo} .\n")
            self.n += 1


def lit(s):
    return f'"{esc(s)}"'


def langlit(s, tag):
    return f'"{esc(s)}"@{tag}'


def typed(v, dt):
    return f'"{v}"^^<{dt}>'


def main():
    w = Writer(OUT)
    date_o = typed(ADDED, XSD + "date")

    # --- 1. multilingual vernacular names (RDF-star provenance) ---
    n_names = 0
    n_sp_named = 0
    for line in open(VERN, encoding="utf-8"):
        if not line.strip():
            continue
        r = json.loads(line)
        key = r["key"]
        s = sp_iri(key)
        api = f"https://api.gbif.org/v1/species/{key}/vernacularNames"
        got = False
        for code, rec in r["names"].items():
            name = (rec.get("name") or "").strip()
            if not name:
                continue
            tag = LANG.get(code, code)
            src = rec.get("source") or "GBIF vernacular names"
            w.annotate(s, VERN_NAME, langlit(name, tag), [
                (SOURCE, lit(src)),
                (GENERATED, date_o),
                (DERIVED, f"<{api}>"),
            ])
            n_names += 1
            got = True
        if got:
            n_sp_named += 1
    print(f"names: {n_names:,} vernacular names on {n_sp_named:,} species", file=sys.stderr)

    # --- 2. IUCN Red List status + sensitive flag (RDF-star provenance) ---
    n_iucn = 0
    n_sens = 0
    for line in open(IUCN, encoding="utf-8"):
        if not line.strip():
            continue
        r = json.loads(line)
        s = sp_iri(r["key"])
        wd = r.get("wd", "")
        prov = [
            (SOURCE, lit("IUCN Red List")),
            (GENERATED, date_o),
        ]
        if wd:
            prov.append((DERIVED, f"<{wd}>"))
        w.annotate(s, IUCN_STATUS, lit(r["status"]), prov)
        n_iucn += 1
        if r.get("sensitive"):
            w.annotate(s, SENSITIVE_P, typed("true", XSD + "boolean"), [
                (SOURCE, lit("IUCN Red List")),
                (GENERATED, date_o),
            ])
            n_sens += 1
    print(f"iucn: {n_iucn:,} status + {n_sens:,} sensitive flags", file=sys.stderr)

    # --- 3. per-species occurrence aggregates (RDF-star provenance) ---
    con = duckdb.connect()
    files = sorted(glob.glob(PARTS))
    rows = con.execute(f"""
        SELECT specieskey,
               count(*)                                        AS rc,
               sum(CASE WHEN individualcount > 0 THEN individualcount ELSE 0 END) AS est,
               min(year)                                       AS fy,
               max(year)                                       AS ly
        FROM read_parquet({files!r})
        WHERE specieskey IS NOT NULL
        GROUP BY specieskey
    """).fetchall()
    agg_prov = [
        (SOURCE, lit("GBIF occurrences (Spain + Switzerland)")),
        (GENERATED, date_o),
    ]
    n_agg = 0
    for specieskey, rc, est, fy, ly in rows:
        s = sp_iri(specieskey)
        w.annotate(s, REC_COUNT, typed(int(rc), XSD + "integer"), agg_prov)
        if est and int(est) > 0:
            w.annotate(s, EST_INDIV, typed(int(est), XSD + "integer"), agg_prov)
        if fy is not None:
            w.annotate(s, FIRST_YEAR, typed(int(fy), XSD + "gYear"), agg_prov)
        if ly is not None:
            w.annotate(s, LAST_YEAR, typed(int(ly), XSD + "gYear"), agg_prov)
        n_agg += 1
    print(f"aggregates: {n_agg:,} species with occurrence stats", file=sys.stderr)

    w.fh.close()
    print(f"enrich: {w.n:,} triples -> {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
