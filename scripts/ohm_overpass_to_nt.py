#!/usr/bin/env python3
"""OpenHistoricalMap Overpass ([out:json] with `out geom;`) -> atlas N-Triples.

Reads Overpass JSON on stdin, writes N-Triples on stdout in the atlas shape
(matching scripts/geo_to_rete.py):

  <x>      a            <http://ex/OhmFeature> ;
           rdfs:label   "NAME"@en ;
           ex:startYear  SY ;   # xsd:integer signed year (negative = BCE)
           ex:endYear    EY ;   # xsd:integer; 2100 sentinel = "still present"
           geo:hasGeometry <x/geom> .
  <x/geom> geo:asWKT     "WKT"^^geo:wktLiteral .

where x = https://www.openhistoricalmap.org/<type>/<id>.

Run:  python3 scripts/ohm_overpass_to_nt.py < overpass.json > atlas.nt
"""
from __future__ import annotations

import json
import re
import sys

GEO = "http://www.opengis.net/ont/geosparql#"
WKT_DT = f"{GEO}wktLiteral"
EX = "http://ex/"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"

YEAR_MIN, YEAR_MAX = -10000, 2100
END_SENTINEL = 2100  # end_date absent/empty/unparseable => "still present"

# --------------------------------------------------------------------------
# (1) DATE PARSING (OSM/EDTF -> signed integer year)
# --------------------------------------------------------------------------
# Seen on OHM start_date/end_date: '1815', '1815-06-18', '1815-06',
# '-0500'/'-500' (BCE), with qualifiers '~' '?' 'ca.' 'c.' 'circa'
# 'early '/'mid '/'late ', or ranges like '1914..1918' / '1914/1918' /
# '1914-1918'. We extract the FIRST signed integer year.
_QUALIFIER_RE = re.compile(
    r"\b(?:ca\.?|c\.?|circa|early|mid|late|before|after|aprox\.?|approx\.?)\b",
    re.I,
)
# Leading optional minus, optional leading zeros, then 1-5 digits, as the first
# numeric run (the lookbehind keeps us off the '18' of a day / a decimal tail).
_YEAR_RE = re.compile(r"(?<![\d.])(-?)0*(\d{1,5})")


def parse_year(s):
    """Extract a SIGNED integer year from an OSM/EDTF date string, or None.

      * strip qualifier words and stray '~'/'?'
      * leading minus on the first numeric token => BCE => negative
        ('-0500' -> -500, '-500' -> -500)
      * range ('1914..1918', '1914/1918', '1914-1918') -> FIRST year
      * leading zeros dropped ('0079' -> 79)
      * clamp to [-10000, 2100]
    """
    if not s:
        return None
    s = str(s).strip()
    if not s:
        return None
    s = s.replace("~", " ").replace("?", " ")
    s = _QUALIFIER_RE.sub(" ", s)
    m = _YEAR_RE.search(s)
    if not m:
        return None
    sign, digits = m.group(1), m.group(2)
    year = int(digits)
    if sign == "-":
        year = -year
    if year < YEAR_MIN:
        year = YEAR_MIN
    if year > YEAR_MAX:
        year = YEAR_MAX
    return year


def years_of(tags):
    """(start_year, end_year), or (None, None) when start is unparseable.

    end_year falls back to END_SENTINEL (2100) when end_date is absent, empty,
    or unparseable."""
    sy = parse_year(tags.get("start_date"))
    if sy is None:
        return None, None
    ey = parse_year(tags.get("end_date"))
    if ey is None:
        ey = END_SENTINEL
    return sy, ey


# --------------------------------------------------------------------------
# (2) GEOMETRY -> WKT  (lon lat, space-separated, no SRID)
# --------------------------------------------------------------------------
def _fmt(lon, lat):
    return f"{lon} {lat}"


def _coords_pairs(geometry):
    """[{lat,lon}, ...] -> [(lon, lat), ...], dropping malformed points."""
    out = []
    for p in geometry or []:
        if isinstance(p, dict) and "lat" in p and "lon" in p:
            out.append((p["lon"], p["lat"]))
    return out


def _is_closed(pairs):
    return len(pairs) >= 4 and pairs[0] == pairs[-1]


def node_wkt(el):
    lon, lat = el.get("lon"), el.get("lat")
    if lon is None or lat is None:
        return None
    return f"POINT({_fmt(lon, lat)})"


def way_wkt(el):
    """Way with an `out geom` 'geometry' array.

    AREA (=> POLYGON) when the ring is closed (first==last) OR the way is
    area-tagged (area=yes, or building/landuse/leisure/natural/boundary/amenity
    present) with >=3 points; otherwise LINESTRING."""
    pairs = _coords_pairs(el.get("geometry"))
    if len(pairs) < 2:
        return None
    tags = el.get("tags") or {}
    area_tagged = tags.get("area") == "yes" or any(
        k in tags
        for k in ("building", "landuse", "leisure", "natural", "boundary", "amenity")
    )
    closed = _is_closed(pairs)
    if closed or (area_tagged and len(pairs) >= 3):
        ring = list(pairs)
        if ring[0] != ring[-1]:
            ring.append(ring[0])  # close the ring
        if len(ring) < 4:  # too small to be a valid polygon ring
            return f"LINESTRING({', '.join(_fmt(x, y) for x, y in pairs)})"
        return f"POLYGON(({', '.join(_fmt(x, y) for x, y in ring)}))"
    return f"LINESTRING({', '.join(_fmt(x, y) for x, y in pairs)})"


def _ring_str(pairs):
    """A closed ring '(lon lat, ...)' or None if degenerate."""
    ring = list(pairs)
    if len(ring) < 3:
        return None
    if ring[0] != ring[-1]:
        ring.append(ring[0])
    if len(ring) < 4:
        return None
    return "(" + ", ".join(_fmt(x, y) for x, y in ring) + ")"


def relation_wkt(el):
    """Relation / multipolygon.

    If members carry geometry, assemble a MULTIPOLYGON: each outer member starts
    a polygon; inner members become holes on the most recent outer (order-based
    pairing — Overpass emits each outer with its inners contiguously). If no
    usable ring geometry exists, fall back to a representative POINT: the
    centroid of all member coords, else the centre of the 'bounds' box."""
    members = el.get("members") or []
    polygons = []   # list of [outer_ring_str, hole_str, ...]
    all_pairs = []
    for m in members:
        if m.get("type") != "way":
            continue
        pairs = _coords_pairs(m.get("geometry"))
        all_pairs.extend(pairs)
        rs = _ring_str(pairs)
        if rs is None:
            continue
        role = m.get("role") or ""
        if role == "inner" and polygons:
            polygons[-1].append(rs)        # hole on current polygon
        else:                              # 'outer' / '' / other => new polygon
            polygons.append([rs])

    if polygons:
        polys = ["(" + ", ".join(p) + ")" for p in polygons]
        if len(polys) == 1:
            return f"POLYGON{polys[0]}"
        return "MULTIPOLYGON(" + ", ".join(polys) + ")"

    # Fallback: representative point.
    if all_pairs:
        cx = sum(x for x, _ in all_pairs) / len(all_pairs)
        cy = sum(y for _, y in all_pairs) / len(all_pairs)
        return f"POINT({_fmt(cx, cy)})"
    b = el.get("bounds")
    if b and all(k in b for k in ("minlon", "maxlon", "minlat", "maxlat")):
        cx = (b["minlon"] + b["maxlon"]) / 2.0
        cy = (b["minlat"] + b["maxlat"]) / 2.0
        return f"POINT({_fmt(cx, cy)})"
    return None


def element_wkt(el):
    t = el.get("type")
    if t == "node":
        return node_wkt(el)
    if t == "way":
        return way_wkt(el)
    if t == "relation":
        return relation_wkt(el)
    return None


# --------------------------------------------------------------------------
# (3) N-Triples emission
# --------------------------------------------------------------------------
def esc(s):
    """Escape a string for an N-Triples literal (backslash FIRST, then the rest)."""
    return (
        s.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
    )


def emit(out, el):
    tags = el.get("tags") or {}
    name = tags.get("name") or tags.get("name:en")
    if not name or not name.strip():
        return False                       # skip: no label
    sy, ey = years_of(tags)
    if sy is None:
        return False                       # skip: no parseable start year
    wkt = element_wkt(el)
    if not wkt:
        return False                       # skip: no geometry
    iri = f"https://www.openhistoricalmap.org/{el['type']}/{el['id']}"
    g = f"{iri}/geom"
    out.write(f"<{iri}> <{RDF_TYPE}> <{EX}OhmFeature> .\n")
    out.write(f'<{iri}> <{RDFS_LABEL}> "{esc(name.strip())}"@en .\n')
    out.write(f'<{iri}> <{EX}startYear> "{sy}"^^<{XSD_INT}> .\n')
    out.write(f'<{iri}> <{EX}endYear> "{ey}"^^<{XSD_INT}> .\n')
    out.write(f"<{iri}> <{GEO}hasGeometry> <{g}> .\n")
    out.write(f'<{g}> <{GEO}asWKT> "{esc(wkt)}"^^<{WKT_DT}> .\n')
    return True


def main():
    data = json.load(sys.stdin)
    out = sys.stdout
    n = kept = 0
    for el in data.get("elements", []):
        n += 1
        try:
            if emit(out, el):
                kept += 1
        except (KeyError, TypeError, ValueError):
            continue  # robust: skip any malformed element
    print(f"ohm-overpass: kept {kept}/{n} elements", file=sys.stderr)


if __name__ == "__main__":
    main()
