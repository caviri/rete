#!/usr/bin/env python3
"""Geo-enrich the arxiu graph: municipal POINTs (Wikidata) + province links (geoadmin IRIs).

Reads data/arxiu/records_*.jsonl, extracts "Terme municipal de X" / "Comarca de Y"
from titles, matches X against the 951 Catalan municipalities fetched from WDQS
(data/arxiu/cat_municipis.json), and writes data/arxiu/arxiu_geo.nt with, per unit:
  <unit> geo:asWKT "POINT(lon lat)"^^geo:wktLiteral   (standalone Map view)
  <unit> schema:contentLocation <geoadmin province IRI> (TRUE federation join key)
  <unit> schema:containedInPlace <wikidata municipality> (round-trip)
Rebuild with:  rete build data/arxiu/arxiu.nt data/arxiu/arxiu_geo.nt -o ... --card
"""
import json, re, glob, unicodedata
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DATA = REPO / "data" / "arxiu"
OUT = DATA / "arxiu_geo.nt"

GEO = "http://www.opengis.net/ont/geosparql#"
SCH = "http://schema.org/"
UNIT = "https://w3id.org/rete/arxiu/unit/"

PROV_IRI = {  # geoadmin district IRIs (the federation join keys)
    "barcelona": "https://w3id.org/rete/geoadmin/district/93216281B38817874256440",
    "girona": "https://w3id.org/rete/geoadmin/district/93216281B12743856325627",
    "lleida": "https://w3id.org/rete/geoadmin/district/93216281B77878961012946",
    "tarragona": "https://w3id.org/rete/geoadmin/district/93216281B93269883088590",
}
COMARCA_PROV = {  # comarca -> province (normalized keys)
    **{c: "barcelona" for c in ["alt penedes", "anoia", "bages", "baix llobregat", "barcelones",
        "bergueda", "garraf", "maresme", "moianes", "osona", "valles occidental", "valles oriental"]},
    **{c: "girona" for c in ["alt emporda", "baix emporda", "garrotxa", "girones",
        "pla de l estany", "ripolles", "selva", "cerdanya"]},
    **{c: "lleida" for c in ["alta ribagorca", "alt urgell", "garrigues", "noguera",
        "pallars jussa", "pallars sobira", "pla d urgell", "segarra", "segria",
        "solsones", "urgell", "aran", "val d aran"]},
    **{c: "tarragona" for c in ["alt camp", "baix camp", "baix ebre", "baix penedes",
        "conca de barbera", "montsia", "priorat", "ribera d ebre", "tarragones", "terra alta"]},
}
ART = re.compile(r"^(els |les |el |la |l |l')")

def norm(s):
    s = unicodedata.normalize("NFKD", s.lower()).encode("ascii", "ignore").decode()
    s = re.sub(r"[''’-]", " ", s)
    s = re.sub(r"\s+", " ", s).strip()
    return ART.sub("", s).strip()

# municipality name -> (wd IRI, lon, lat)
munis = {}
for b in json.load(open(DATA / "cat_municipis.json", encoding="utf-8"))["results"]["bindings"]:
    m = re.match(r"Point\(([-\d.]+) ([-\d.]+)\)", b["coord"]["value"])
    if m:
        munis[norm(b["mLabel"]["value"])] = (b["m"]["value"], m.group(1), m.group(2))
# municipality -> comarca (for the province hop)
muni_com = {}
for b in json.load(open(DATA / "cat_muni_comarca.json", encoding="utf-8"))["results"]["bindings"]:
    muni_com[b["m"]["value"]] = norm(b["cLabel"]["value"])

PAT_TM = re.compile(r"Terme municipal (?:de |d')([^.]+?)\s*[.,]")
PAT_CO = re.compile(r"Comarca (?:de la |del |de l'|de |d')([A-Za-zÀ-ú' ]+)")

seen, n_geo, n_prov = set(), 0, 0
with open(OUT, "w", encoding="utf-8") as out:
    for f in sorted(glob.glob(str(DATA / "records_*.jsonl"))):
        for line in open(f, encoding="utf-8"):
            r = json.loads(line)
            ref = r.get("codiReferencia")
            if not ref or ref in seen:
                continue
            text = (r.get("titol") or "") + ". " + (r.get("descripcio") or "")
            m = PAT_TM.search(text)
            if not m:
                continue
            hit = munis.get(norm(m.group(1)))
            if not hit:
                continue
            seen.add(ref)
            wd, lon, lat = hit
            u = f"{UNIT}{ref}"
            out.write(f'<{u}> <{GEO}asWKT> "POINT({lon} {lat})"^^<{GEO}wktLiteral> .\n')
            out.write(f"<{u}> <{SCH}containedInPlace> <{wd}> .\n")
            n_geo += 1
            # province: via muni's comarca, else the title's own "Comarca de Y"
            com = muni_com.get(wd)
            if not com:
                mc = PAT_CO.search(text)
                com = norm(mc.group(1)) if mc else None
            prov = COMARCA_PROV.get(com) if com else None
            if prov:
                out.write(f"<{u}> <{SCH}contentLocation> <{PROV_IRI[prov]}> .\n")
                n_prov += 1
print(f"matched units: {n_geo} (with province link: {n_prov}) -> {OUT}")
