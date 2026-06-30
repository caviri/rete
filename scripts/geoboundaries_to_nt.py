#!/usr/bin/env python3
"""geoBoundaries (OSM-derived, CC-BY) global admin boundaries -> GeoSPARQL N-Triples,
the "big" geoadmin graph for federated geospatial joins.

ADM0 countries, ADM1 regions, ADM2 districts as polygons (geo:asWKT), plus Natural
Earth populated places as points. geoBoundaries codes countries by ISO alpha-3
(shapeGroup); we map to alpha-2 (g:iso) via the datasets/country-codes table so a
federation partner can join on a plain dwc:countryCode ("ES").

Inputs in data/geoadmin/: gb_adm0.geojson, gb_adm1.geojson, gb_adm2.geojson,
country-codes.csv, places50m.geojson (optional, for points).
Usage: python3 scripts/geoboundaries_to_nt.py > data/geoadmin/geoadmin.nt
"""
import csv, json, os, sys, urllib.parse

sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(HERE, "data", "geoadmin")

BASE = "https://geoadmin.rete/"
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


# Detailed OSM polygons are too heavy to ship + render (Spain ~1 MB, max ~21 MB of
# WKT). Simplify with Douglas–Peucker (iterative, no recursion limit) — at a ~1 km
# tolerance a boundary is visually identical on a map but ~10-30x fewer points, so
# the .rete shrinks and the browser parses each cell instantly.
#
# Multi-LOD: every admin area ships TWO resolutions. `geo:asWKT` is the coarse
# overview used in the table cell + the locator map + federation. `g:geomFine` is a
# finer LOD (only Country/Region — the large features where coarse is visible). The
# playground fetches g:geomFine for JUST the feature you open in the map modal — the
# remote-lazy payoff: zoom in → fetch only that one polygon's detail, not the corpus.
SIMPLIFY_EPS = 0.01    # degrees (~1.1 km) — coarse, the default geo:asWKT
FINE_EPS = 0.001       # degrees (~110 m) — the g:geomFine LOD, fetched on demand

def dp(pts, eps):
    n = len(pts)
    if n <= 2:
        return pts
    keep = [False] * n
    keep[0] = keep[n - 1] = True
    eps2 = eps * eps
    stack = [(0, n - 1)]
    while stack:
        i, j = stack.pop()
        if j <= i + 1:
            continue
        ax, ay = pts[i]; bx, by = pts[j]
        dx, dy = bx - ax, by - ay
        ll = dx * dx + dy * dy
        dmax, idx = -1.0, -1
        for k in range(i + 1, j):
            px, py = pts[k]
            if ll == 0:
                d = (px - ax) ** 2 + (py - ay) ** 2
            else:
                tcl = ((px - ax) * dx + (py - ay) * dy) / ll
                d = (px - (ax + tcl * dx)) ** 2 + (py - (ay + tcl * dy)) ** 2
            if d > dmax:
                dmax, idx = d, k
        if dmax > eps2:
            keep[idx] = True
            stack.append((i, idx)); stack.append((idx, j))
    return [pts[k] for k in range(n) if keep[k]]

def simp(cs, eps=SIMPLIFY_EPS):
    return dp([(c[0], c[1]) for c in cs if len(c) >= 2], eps)
def fmt(pts):
    return ", ".join("%.4f %.4f" % (x, y) for x, y in pts)
def poly_body(rings, eps=SIMPLIFY_EPS):
    parts = [fmt(p) for p in (simp(r, eps) for r in rings) if len(p) >= 4]  # drop degenerate rings
    return ", ".join("(%s)" % p for p in parts) if parts else None
def geom_wkt(g, eps=SIMPLIFY_EPS):
    if not g:
        return None
    typ, c = g.get("type"), g.get("coordinates")
    if typ == "Point":
        return "POINT(%.5f %.5f)" % (c[0], c[1])
    if typ == "Polygon":
        b = poly_body(c, eps); return ("POLYGON(%s)" % b) if b else None
    if typ == "MultiPolygon":
        ps = [b for b in (poly_body(p, eps) for p in c) if b]
        return ("MULTIPOLYGON(%s)" % ", ".join("(%s)" % b for b in ps)) if ps else None
    if typ == "LineString":
        p = simp(c, eps); return ("LINESTRING(%s)" % fmt(p)) if len(p) >= 2 else None
    if typ == "MultiLineString":
        ls = [fmt(p) for p in (simp(r, eps) for r in c) if len(p) >= 2]
        return ("MULTILINESTRING(%s)" % ", ".join("(%s)" % p for p in ls)) if ls else None
    return None


def load_iso2():
    path = os.path.join(DATA, "country-codes.csv")
    m = {}
    if os.path.isfile(path):
        for r in csv.DictReader(open(path, encoding="utf-8")):
            a3, a2 = r.get("ISO3166-1-Alpha-3"), r.get("ISO3166-1-Alpha-2")
            if a3 and a2:
                m[a3] = a2
    return m


ISO2 = load_iso2()


def emit_ontology():
    for cls, label, comment in [
        ("Country", "Country", "A sovereign country (geoBoundaries ADM0)."),
        ("Region", "Region / state", "A first-level admin area (geoBoundaries ADM1)."),
        ("District", "District / county", "A second-level admin area (geoBoundaries ADM2)."),
        ("Place", "Populated place", "A city / town as a point (Natural Earth)."),
    ]:
        t(C + cls, RDF, iri(OWL + "Class")); tl(C + cls, LBL, label); tl(C + cls, RDFS + "comment", comment)
    for pid, label in [("name", "name"), ("iso", "ISO 3166-1 alpha-2 code"), ("iso3", "ISO alpha-3 code"),
                       ("adminLevel", "admin level"), ("country", "country code"), ("partOf", "part of"),
                       ("population", "population (est.)"), ("geomFine", "detailed geometry (fine LOD, ~110 m)")]:
        tl(P + pid, LBL, label)
    for pid in ("country", "partOf"):
        t(P + pid, RDF, iri(OWL + "ObjectProperty"))


def emit_admin(fname, cls, level):
    path = os.path.join(DATA, fname)
    if not os.path.isfile(path):
        sys.stderr.write("missing %s — skipped\n" % fname); return 0
    d = json.load(open(path, encoding="utf-8"))
    n = 0
    for f in d.get("features", []):
        p = f.get("properties", {})
        wkt = geom_wkt(f.get("geometry"))
        name = p.get("shapeName")
        grp = p.get("shapeGroup")             # ISO alpha-3
        a2 = ISO2.get(grp)
        if not (name and wkt):
            continue
        if cls == "Country":
            s = BASE + "country/" + urllib.parse.quote(a2 or grp or name, safe="")
            t(s, RDF, iri(C + "Country")); tl(s, LBL, name); tl(s, P + "name", name)
            if a2: tl(s, P + "iso", a2)
            if grp: tl(s, P + "iso3", grp)
            tl(s, P + "adminLevel", "0")
        else:
            sid = p.get("shapeID") or ((grp or "") + "/" + name)
            s = BASE + cls.lower() + "/" + urllib.parse.quote(sid, safe="")
            t(s, RDF, iri(C + cls)); tl(s, LBL, name); tl(s, P + "name", name); tl(s, P + "adminLevel", level)
            if a2:
                tl(s, P + "country", a2)
                t(s, P + "partOf", iri(BASE + "country/" + urllib.parse.quote(a2, safe="")))
        t(s, WKT, "%s^^%s" % (lit(wkt), iri(WKT_DT)))
        # A finer LOD for the big features (countries + regions) — fetched on demand
        # when you open the feature in the map modal. Only when meaningfully finer
        # than the coarse shape, so we don't duplicate already-tiny geometries.
        if cls in ("Country", "Region"):
            fine = geom_wkt(f.get("geometry"), FINE_EPS)
            if fine and len(fine) > len(wkt) + 24:
                t(s, P + "geomFine", "%s^^%s" % (lit(fine), iri(WKT_DT)))
        n += 1
    sys.stderr.write("%s (%s): %d\n" % (cls, fname, n))
    return n


def emit_places():
    path = os.path.join(DATA, "places50m.geojson")
    if not os.path.isfile(path):
        return 0
    d = json.load(open(path, encoding="utf-8"))
    n = 0
    for f in d.get("features", []):
        p = f.get("properties", {})
        wkt = geom_wkt(f.get("geometry"))
        name = p.get("NAME") or p.get("name")
        if not (name and wkt):
            continue
        s = BASE + "place/" + urllib.parse.quote(name + "/" + str(p.get("ADM0_A3", "")), safe="")
        t(s, RDF, iri(C + "Place")); tl(s, LBL, name); tl(s, P + "name", name)
        if p.get("ISO_A2") and p["ISO_A2"] not in ("-99", "-1", ""):
            tl(s, P + "country", p["ISO_A2"])
        if p.get("POP_MAX"):
            tl(s, P + "population", str(int(p["POP_MAX"])))
        t(s, WKT, "%s^^%s" % (lit(wkt), iri(WKT_DT)))
        n += 1
    sys.stderr.write("Place: %d\n" % n)
    return n


emit_ontology()
emit_admin("gb_adm0.geojson", "Country", "0")
emit_admin("gb_adm1.geojson", "Region", "1")
emit_admin("gb_adm2.geojson", "District", "2")
emit_places()
