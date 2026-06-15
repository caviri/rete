#!/usr/bin/env python3
"""Antarctic territorial claims as dated sector polygons -> atlas N-Triples.

The seven claims are longitude sectors running from 60S to the South Pole, each with
a formal-claim year; British/Argentine/Chilean deliberately overlap on the Peninsula
(the real contested zone). Ross & Australian wrap the antimeridian (MultiPolygon).
Peter I Island is a point claim; Marie Byrd Land is the unclaimed gap.
Sector bounds + claim years are uncopyrightable facts (cross-checked vs Wikipedia);
the wedge geometry is generated. INTERVAL shape (claimYear -> 2100 sentinel).

Usage:  python3 scripts/antarctic_claims.py > data/antarctica/claims.nt
"""
import sys

GEO = "http://www.opengis.net/ont/geosparql#"
EX = "http://ex/"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"
LAT0, LAT1 = -60.0, -89.5   # sector spans from 60S down toward the pole

# key, label, claimYear, geometry spec:
#   ("sector", [(lonW, lonE), ...])  -> Polygon/MultiPolygon wedge(s)
#   ("point", lon, lat)              -> Point
CLAIMS = [
    ("british",   "British Antarctic Territory", 1908, ("sector", [(-80.0, -20.0)])),
    ("argentine", "Argentine Antarctica",         1942, ("sector", [(-74.0, -25.0)])),
    ("chilean",   "Chilean Antarctic Territory",  1940, ("sector", [(-90.0, -53.0)])),
    ("ross",      "Ross Dependency (New Zealand)",1923, ("sector", [(160.0, 180.0), (-180.0, -150.0)])),
    ("australian","Australian Antarctic Territory",1933, ("sector", [(45.0, 136.0), (142.0, 160.0)])),
    ("adelie",    "Adelie Land (France)",         1924, ("sector", [(136.0, 142.0)])),
    ("queenmaud", "Queen Maud Land (Norway)",     1939, ("sector", [(-20.0, 45.0)])),
    ("peter1",    "Peter I Island (Norway)",      1931, ("point", -90.583, -68.85)),
    ("mariebyrd", "Marie Byrd Land (unclaimed)",  1820, ("sector", [(-150.0, -90.0)])),
]


def ring(lon_w, lon_e):
    # rectangle in lon/lat from 60S to near-pole, sampled along the 60S edge so it
    # reads as a band/wedge in every projection. lon first (WKT lon lat).
    steps = 8
    top = [(lon_w + (lon_e - lon_w) * i / steps, LAT0) for i in range(steps + 1)]
    pts = top + [(lon_e, LAT1), (lon_w, LAT1), (lon_w, LAT0)]
    return "(" + ", ".join(f"{x} {y}" for x, y in pts) + ")"


def wkt(spec):
    kind = spec[0]
    if kind == "point":
        return f"Point({spec[1]} {spec[2]})"
    polys = ["(" + ring(w, e) + ")" for (w, e) in spec[1]]
    if len(polys) == 1:
        return "POLYGON" + polys[0]
    return "MULTIPOLYGON(" + ", ".join(polys) + ")"


def main():
    out = sys.stdout
    for key, label, year, spec in CLAIMS:
        x = f"{EX}claim/{key}"
        g = f"{x}/geom"
        out.write(f"<{x}> <{RDF}type> <{EX}Claim> .\n")
        out.write(f'<{x}> <{RDFS}label> "{label}"@en .\n')
        out.write(f'<{x}> <{EX}startYear> "{year}"^^<{XSD_INT}> .\n')
        out.write(f'<{x}> <{EX}endYear> "2100"^^<{XSD_INT}> .\n')
        out.write(f"<{x}> <{GEO}hasGeometry> <{g}> .\n")
        out.write(f'<{g}> <{GEO}asWKT> "{wkt(spec)}"^^<{GEO}wktLiteral> .\n')
    print(f"antarctic-claims: {len(CLAIMS)} claims", file=sys.stderr)


if __name__ == "__main__":
    main()
