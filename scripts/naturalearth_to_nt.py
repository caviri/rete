#!/usr/bin/env python3
"""Natural Earth (public-domain world map) admin boundaries + places -> N-Triples
with GeoSPARQL geometries, for federated geospatial queries.

Builds an `geoadmin` graph: Country / State / Place entities, each with a name, an
ISO code (so it joins another dataset's dwc:countryCode at the term level), an
admin level, and its geometry as geo:asWKT (polygons for areas, a point for
places). Federate it with e.g. bioexplora to attach a country's boundary polygon
to every specimen — "get the geospatial column" by a cross-source join on the code.

Source GeoJSON (CC0 / public domain, Natural Earth via nvkelso/natural-earth-vector):
  data/geoadmin/countries110m.geojson, states50m.geojson, places50m.geojson
Usage: python3 scripts/naturalearth_to_nt.py > data/geoadmin/geoadmin.nt
"""
import json, os, sys, urllib.parse

sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(HERE, "data", "geoadmin")

BASE = "https://natural-earth.rete/"
P = BASE + "prop/"
C = BASE + "class/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
LBL = RDFS + "label"
OWL = "http://www.w3.org/2002/07/owl#"
WKT = "http://www.opengis.net/ont/geosparql#asWKT"
WKT_DT = "http://www.opengis.net/ont/geosparql#wktLiteral"

out = sys.stdout
def w(s): out.write(s + "\n")
def iri(s): return "<" + s + ">"
def esc(s): return str(s).replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ").replace("\r", " ").replace("\t", " ")
def lit(s): return '"' + esc(s) + '"'
def t(s, p, o): w(iri(s) + " " + iri(p) + " " + o + " .")
def tl(s, p, o): t(s, p, lit(o))


def ring(coords):
    return ", ".join("%s %s" % (c[0], c[1]) for c in coords if len(c) >= 2)


def poly_body(rings):  # [outer, hole, …] -> "(a b, …), (hole…)"
    return ", ".join("(%s)" % ring(r) for r in rings if r)


def geom_wkt(g):
    if not g:
        return None
    typ, c = g.get("type"), g.get("coordinates")
    if typ == "Point":
        return "POINT(%s %s)" % (c[0], c[1])
    if typ == "MultiPoint":
        return "MULTIPOINT(%s)" % ", ".join("(%s %s)" % (p[0], p[1]) for p in c)
    if typ == "Polygon":
        return "POLYGON(%s)" % poly_body(c)
    if typ == "MultiPolygon":
        return "MULTIPOLYGON(%s)" % ", ".join("(%s)" % poly_body(poly) for poly in c)
    if typ == "LineString":
        return "LINESTRING(%s)" % ring(c)
    if typ == "MultiLineString":
        return "MULTILINESTRING(%s)" % ", ".join("(%s)" % ring(r) for r in c)
    return None


def feat_iso(p):
    for k in ("ISO_A2", "ISO_A2_EH", "iso_a2"):
        v = p.get(k)
        if v and v not in ("-99", "-1", ""):
            return v
    return None


def load(name):
    path = os.path.join(DATA, name)
    if not os.path.isfile(path):
        sys.stderr.write("missing %s — skipped\n" % name)
        return []
    return json.load(open(path, encoding="utf-8")).get("features", [])


def emit_ontology():
    for cls, label, comment in [
        ("Country", "Country", "A sovereign country / Natural Earth admin-0 area."),
        ("State", "State / province", "A first-level admin area (Natural Earth admin-1)."),
        ("Place", "Populated place", "A city / town as a point."),
    ]:
        t(C + cls, RDF, iri(OWL + "Class")); tl(C + cls, LBL, label); tl(C + cls, RDFS + "comment", comment)
    for pid, label in [("name", "name"), ("iso", "ISO 3166-1 alpha-2 code"), ("iso3", "ISO alpha-3 code"),
                       ("adminLevel", "admin level"), ("continent", "continent"),
                       ("population", "population (est.)"), ("country", "country code"), ("partOf", "part of")]:
        tl(P + pid, LBL, label)
    t(P + "country", RDF, iri(OWL + "ObjectProperty"))
    t(P + "partOf", RDF, iri(OWL + "ObjectProperty"))


def emit_countries():
    n = 0
    for f in load("countries110m.geojson"):
        p = f.get("properties", {})
        iso = feat_iso(p)
        name = p.get("NAME") or p.get("ADMIN") or p.get("NAME_LONG")
        wkt = geom_wkt(f.get("geometry"))
        if not (iso and name and wkt):
            continue
        s = BASE + "country/" + urllib.parse.quote(iso, safe="")
        t(s, RDF, iri(C + "Country")); tl(s, LBL, name); tl(s, P + "name", name)
        tl(s, P + "iso", iso); tl(s, P + "adminLevel", "0")
        if p.get("ISO_A3") and p["ISO_A3"] not in ("-99", "-1"):
            tl(s, P + "iso3", p["ISO_A3"])
        if p.get("CONTINENT"):
            tl(s, P + "continent", p["CONTINENT"])
        if p.get("POP_EST"):
            tl(s, P + "population", str(int(p["POP_EST"])))
        t(s, WKT, '%s^^%s' % (lit(wkt), iri(WKT_DT)))
        n += 1
    sys.stderr.write("countries: %d\n" % n)


def emit_states():
    n = 0
    for f in load("states50m.geojson"):
        p = f.get("properties", {})
        name = p.get("name") or p.get("name_en") or p.get("gn_name")
        wkt = geom_wkt(f.get("geometry"))
        if not (name and wkt):
            continue
        key = (p.get("adm1_code") or (p.get("iso_a2", "") + "/" + name))
        s = BASE + "state/" + urllib.parse.quote(key, safe="")
        t(s, RDF, iri(C + "State")); tl(s, LBL, name); tl(s, P + "name", name); tl(s, P + "adminLevel", "1")
        iso = p.get("iso_a2")
        if iso and iso not in ("-99", "-1", ""):
            tl(s, P + "country", iso)
            t(s, P + "partOf", iri(BASE + "country/" + urllib.parse.quote(iso, safe="")))
        t(s, WKT, '%s^^%s' % (lit(wkt), iri(WKT_DT)))
        n += 1
    sys.stderr.write("states: %d\n" % n)


def emit_places():
    n = 0
    for f in load("places50m.geojson"):
        p = f.get("properties", {})
        name = p.get("NAME") or p.get("name")
        wkt = geom_wkt(f.get("geometry"))
        if not (name and wkt):
            continue
        s = BASE + "place/" + urllib.parse.quote(name + "/" + str(p.get("ADM0_A3", "")), safe="")
        t(s, RDF, iri(C + "Place")); tl(s, LBL, name); tl(s, P + "name", name)
        iso = p.get("ISO_A2")
        if iso and iso not in ("-99", "-1", ""):
            tl(s, P + "country", iso)
        if p.get("POP_MAX"):
            tl(s, P + "population", str(int(p["POP_MAX"])))
        t(s, WKT, '%s^^%s' % (lit(wkt), iri(WKT_DT)))
        n += 1
    sys.stderr.write("places: %d\n" % n)


emit_ontology()
emit_countries()
emit_states()
emit_places()
