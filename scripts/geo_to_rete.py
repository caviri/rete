#!/usr/bin/env python3
"""Convert historical geo datasets to N-Triples with GeoSPARQL geometry + time.

Two sources, one RDF shape (GeoSPARQL: a feature ``geo:hasGeometry`` a geometry
node whose ``geo:asWKT`` is a ``geo:wktLiteral``; time as plain xsd:integer years
so SPARQL can FILTER on them):

  basemaps  historical-basemaps world_<year>.geojson snapshots (MultiPolygon
            world borders); the snapshot year is the validity instant.
  ohm       a CSV exported from the OpenHistoricalMap QLever endpoint
            (columns: s,name,start,end,level,wkt) — real boundaries with
            start/end dates.

Geometry is simplified for the in-browser playground: coordinates are rounded to
``--prec`` decimals (2 ≈ 1 km, plenty for country-level point-in-polygon),
consecutive duplicate points are dropped, degenerate rings removed, and polygons
whose bounding box is smaller than ``--min-bbox`` square degrees are dropped
(tiny islands). The full-precision data stays in dev/geo/ for a larger build.

Usage:
  geo_to_rete.py basemaps --dir dev/geo/basemaps --years 1,1000,1492,1815,1914,1945,1994 \
      --prec 2 --min-bbox 0.2 -o out.nt
  geo_to_rete.py ohm --csv dev/geo/ohm-admin.csv --prec 4 -o ohm.nt
"""
from __future__ import annotations

import argparse
import csv
import json
import re
import sys
from pathlib import Path

GEO = "http://www.opengis.net/ont/geosparql#"
WKT_DT = f"{GEO}wktLiteral"
EX = "http://ex/"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"

csv.field_size_limit(1 << 27)


def slug(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "_", s.strip()).strip("_") or "x"


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def ring_wkt(ring, prec):
    """One polygon ring -> 'lon lat, ...' with rounding + consecutive dedup, or
    None if degenerate (fewer than 4 distinct points incl. closing)."""
    pts = []
    last = None
    for lon, lat in ring:
        p = (round(lon, prec), round(lat, prec))
        if p != last:
            pts.append(p)
            last = p
    if len(pts) >= 1 and pts[0] != pts[-1]:
        pts.append(pts[0])  # close the ring
    if len(pts) < 4:
        return None
    return "(" + ", ".join(f"{x} {y}" for x, y in pts) + ")"


def bbox_area(coords):
    xs = [c[0] for poly in coords for ring in poly for c in ring]
    ys = [c[1] for poly in coords for ring in poly for c in ring]
    if not xs:
        return 0.0
    return (max(xs) - min(xs)) * (max(ys) - min(ys))


def polygon_wkt(rings, prec):
    parts = [ring_wkt(r, prec) for r in rings]
    parts = [p for p in parts if p]
    return "(" + ", ".join(parts) + ")" if parts else None


def geom_to_wkt(geom, prec, min_bbox):
    t = geom["type"]
    if t == "Polygon":
        coords = [geom["coordinates"]]
    elif t == "MultiPolygon":
        coords = geom["coordinates"]
    else:
        return None
    if bbox_area(coords) < min_bbox:
        return None
    polys = [polygon_wkt(rings, prec) for rings in coords]
    polys = [p for p in polys if p]
    if not polys:
        return None
    if len(polys) == 1:
        return f"POLYGON{polys[0]}"
    return "MULTIPOLYGON(" + ", ".join(polys) + ")"


def triple(s, p, o):
    return f"<{s}> <{p}> {o} .\n"


def lit(v, dt=None):
    return f'"{esc(v)}"^^<{dt}>' if dt else f'"{esc(v)}"'


def emit_feature(out, iri, name, year_props, wkt, extra):
    out.write(triple(iri, RDF_TYPE, f"<{EX}Territory>"))
    out.write(triple(iri, RDFS_LABEL, lit(name)))
    for p, v in extra.items():
        if v:
            out.write(triple(iri, f"{EX}{p}", lit(v)))
    for p, y in year_props.items():
        out.write(triple(iri, f"{EX}{p}", lit(str(y), XSD_INT)))
    g = f"{iri}/geom"
    out.write(triple(iri, f"{GEO}hasGeometry", f"<{g}>"))
    out.write(triple(g, f"{GEO}asWKT", lit(wkt, WKT_DT)))


def do_basemaps(args, out):
    years = [y.strip() for y in args.years.split(",") if y.strip()]
    n = 0
    for y in years:
        fn = Path(args.dir) / f"world_{y}.geojson"
        if not fn.exists():
            print(f"  skip (missing): {fn}", file=sys.stderr)
            continue
        year_int = -int(y[2:]) if y.startswith("bc") else int(y)
        try:
            data = json.loads(fn.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, UnicodeDecodeError) as e:
            print(f"  skip (bad json): {fn.name} ({e})", file=sys.stderr)
            continue
        # Collect (bbox-area, feature) so we can keep the largest N per snapshot
        # (the major territories a point-in-polygon demo cares about).
        feats = []
        for f in data["features"]:
            geom = f["geometry"]
            coords = (
                [geom["coordinates"]] if geom["type"] == "Polygon"
                else geom["coordinates"] if geom["type"] == "MultiPolygon" else None
            )
            if coords is None:
                continue
            wkt = geom_to_wkt(geom, args.prec, args.min_bbox)
            if not wkt:
                continue
            props = f.get("properties", {})
            name = (props.get("NAME") or props.get("name") or "").strip()
            if not name or name.lower() in ("unknown", "unclaimed", "uninhabited"):
                continue  # skip unnamed regions — they only add noise to the map/legend
            feats.append((bbox_area(coords), name, wkt, props))
        feats.sort(key=lambda t: t[0], reverse=True)
        if args.max_per_year:
            feats = feats[: args.max_per_year]
        for _, name, wkt, props in feats:
            iri = f"{EX}terr/{slug(name)}_{y}"
            emit_feature(
                out, iri, name, {"year": year_int},
                wkt, {"subjectTo": props.get("SUBJECTO"), "partOf": props.get("PARTOF")},
            )
            n += 1
        print(f"  {fn.name}: {len(feats)} features (year {year_int})", file=sys.stderr)
    print(f"basemaps: {n} features", file=sys.stderr)


def year_of(s):
    m = re.match(r"\s*(-?\d+)", s or "")
    return int(m.group(1)) if m else None


def do_ohm(args, out):
    n = 0
    with open(args.csv, newline="", encoding="utf-8") as fh:
        for row in csv.DictReader(fh):
            wkt = (row.get("wkt") or "").strip()
            if not (wkt.upper().startswith("POLYGON") or wkt.upper().startswith("MULTIPOLYGON")):
                continue
            # Re-round the (already-WKT) coordinates for size.
            wkt = re.sub(r"-?\d+\.\d+", lambda m: f"{round(float(m.group()), args.prec)}", wkt)
            iri = row["s"]
            ys = {}
            if (s := year_of(row.get("start"))) is not None:
                ys["startYear"] = s
            if (e := year_of(row.get("end"))) is not None:
                ys["endYear"] = e
            emit_feature(out, iri, row.get("name") or "Unknown", ys, wkt,
                         {"adminLevel": row.get("level")})
            n += 1
    print(f"ohm: {n} features", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("basemaps")
    b.add_argument("--dir", default="dev/geo/basemaps")
    b.add_argument("--years", required=True)
    b.add_argument("--prec", type=int, default=2)
    b.add_argument("--min-bbox", type=float, default=0.2)
    b.add_argument("--max-per-year", type=int, default=0, help="keep the N largest features per snapshot (0 = all)")
    b.add_argument("-o", "--out", required=True)
    o = sub.add_parser("ohm")
    o.add_argument("--csv", default="dev/geo/ohm-admin.csv")
    o.add_argument("--prec", type=int, default=4)
    o.add_argument("-o", "--out", required=True)
    args = ap.parse_args()
    with open(args.out, "w", encoding="utf-8") as out:
        (do_basemaps if args.cmd == "basemaps" else do_ohm)(args, out)


if __name__ == "__main__":
    main()
