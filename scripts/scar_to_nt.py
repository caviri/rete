#!/usr/bin/env python3
"""SCAR Composite Gazetteer of Antarctica (WFS CSV, CC-BY 4.0) -> atlas N-Triples.

The authoritative Antarctic place-name dataset (~39k name records, ~19k distinct
features). Undated by nature, so modelled as a static basemap INTERVAL [1820, 2100]
(the era Antarctica has been explored/named) — no per-feature date is fabricated.
Deduped to one point per scar_common_id (the feature), keeping the gazetteer name.

  <http://ex/scar/{id}> a ex:AntarcticPlace ; rdfs:label "{name}"@en ;
      ex:startYear 1820 ; ex:endYear 2100 ; geo:hasGeometry <…/geom> .
  <…/geom> geo:asWKT "Point(lon lat)"^^geo:wktLiteral .

Usage:  python3 scripts/scar_to_nt.py scar_cga.csv > data/antarctica/places.nt
"""
import csv
import sys

GEO = "http://www.opengis.net/ont/geosparql#"
EX = "http://ex/"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ").replace("\r", " ").replace("\t", " ")


def main():
    csv.field_size_limit(10 ** 7)
    out, seen = sys.stdout, set()
    kept = 0
    with open(sys.argv[1], encoding="utf-8-sig", newline="") as fh:
        for r in csv.DictReader(fh):
            name = (r.get("place_name_gazetteer") or "").strip()
            cid = (r.get("scar_common_id") or "").strip()
            lat, lon = (r.get("latitude") or "").strip(), (r.get("longitude") or "").strip()
            if not name or not cid or not lat or not lon or cid in seen:
                continue
            try:
                latf = float(lat); lonf = float(lon)
            except ValueError:
                continue
            if latf > -60:
                continue
            seen.add(cid)
            x = f"{EX}scar/{cid}"
            g = f"{x}/geom"
            out.write(f"<{x}> <{RDF}type> <{EX}AntarcticPlace> .\n")
            out.write(f'<{x}> <{RDFS}label> "{esc(name)}"@en .\n')
            out.write(f'<{x}> <{EX}startYear> "1820"^^<{XSD_INT}> .\n')
            out.write(f'<{x}> <{EX}endYear> "2100"^^<{XSD_INT}> .\n')
            out.write(f"<{x}> <{GEO}hasGeometry> <{g}> .\n")
            out.write(f'<{g}> <{GEO}asWKT> "Point({lonf} {latf})"^^<{GEO}wktLiteral> .\n')
            kept += 1
    print(f"scar: {kept} distinct features", file=sys.stderr)


if __name__ == "__main__":
    main()
