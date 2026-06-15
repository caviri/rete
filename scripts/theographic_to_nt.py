#!/usr/bin/env python3
"""Theographic Bible Metadata (CC BY-SA 4.0) CSV -> atlas N-Triples.

Joins Events.csv (title, startDate year [BCE negative], comma-separated `locations`
of Place keys) to Places.csv (latitude/longitude + kjvName) and emits one INSTANT
biblical event per (event x located place):

  <http://ex/bible/<eventID>_<i>> a <http://ex/BibleEvent> ; rdfs:label "Title @ Place"@en ;
      <http://ex/year> Y ; geo:hasGeometry <.../geom> .  <.../geom> geo:asWKT "Point(lon lat)"^^wktLiteral .

Usage:  python3 scripts/theographic_to_nt.py Places.csv Events.csv > theographic.nt
"""
import csv
import sys

GEO = "http://www.opengis.net/ont/geosparql#"
EX = "http://ex/"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")


def main():
    places_csv, events_csv = sys.argv[1], sys.argv[2]
    places = {}
    for r in csv.DictReader(open(places_csv, encoding="utf-8-sig")):
        key = (r.get("placeLookup") or r.get("slug") or "").strip()
        if not key:
            continue
        lat = (r.get("latitude") or "").strip()
        lon = (r.get("longitude") or "").strip()
        name = (r.get("kjvName") or r.get("esvName") or r.get("displayTitle") or "").strip()
        if lat and lon and name:
            places[key] = (lat, lon, name)

    out = sys.stdout
    kept = 0
    for e in csv.DictReader(open(events_csv, encoding="utf-8-sig")):
        sd = (e.get("startDate") or "").strip()
        loc = (e.get("locations") or "").strip()
        title = (e.get("title") or "").strip()
        eid = (e.get("eventID") or "").strip()
        if not sd or not loc or not title or not eid:
            continue
        try:
            yr = int(float(sd))
        except ValueError:
            continue
        for i, lk in enumerate(x.strip() for x in loc.split(",") if x.strip()):
            p = places.get(lk)
            if not p:
                continue
            lat, lon, pname = p
            try:
                float(lat); float(lon)
            except ValueError:
                continue
            x = f"{EX}bible/{eid}_{i}"
            g = f"{x}/geom"
            label = f"{title} @ {pname}"
            out.write(f"<{x}> <{RDF_TYPE}> <{EX}BibleEvent> .\n")
            out.write(f'<{x}> <{RDFS_LABEL}> "{esc(label)}"@en .\n')
            out.write(f'<{x}> <{EX}year> "{yr}"^^<{XSD_INT}> .\n')
            out.write(f"<{x}> <{GEO}hasGeometry> <{g}> .\n")
            out.write(f'<{g}> <{GEO}asWKT> "Point({lon} {lat})"^^<{GEO}wktLiteral> .\n')
            kept += 1
    print(f"theographic: {kept} events emitted", file=sys.stderr)


if __name__ == "__main__":
    main()
