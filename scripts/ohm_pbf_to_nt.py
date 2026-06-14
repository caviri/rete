#!/usr/bin/env python3
"""OpenHistoricalMap PLANET DUMP (.osm.pbf) -> atlas N-Triples (FULL snapshot).

Streams an entire OHM daily planet file with PyOsmium (>= 4.0), assembling real
geometry, and emits the SAME atlas GeoSPARQL shape as scripts/ohm_overpass_to_nt.py
(whose date-parsing + escaping it reuses verbatim, so the shape stays identical):

  <x>      a            <http://ex/OhmFeature> ;
           rdfs:label   "NAME"@en ;
           ex:startYear  SY ;   # signed xsd:integer (negative = BCE)
           ex:endYear    EY ;   # 2100 sentinel = "still present"
           geo:hasGeometry <x/geom> .
  <x/geom> geo:asWKT     "WKT"^^geo:wktLiteral .

x = https://www.openhistoricalmap.org/<node|way|relation>/<id>.

Unlike the capped Overpass fetcher (scripts/fetch_ohm.sh, ~5,300 elements), this
ingests EVERY dated + named + geometried feature in the planet (~1.0M kept of
~3.3M dated as of 2026-06; the rest are dated-but-unnamed footprints/segments,
and the atlas renders by label). A feature is kept iff it has: a name (or
name:en), a parseable start_date (plain or start_date:edtf), and a valid geometry.

Geometry rules (dup-free, mirrors osmium's area model):
  node                         -> POINT
  open way (not closed)        -> LINESTRING
  area (closed area-tagged way -> POLYGON / MULTIPOLYGON
        or multipolygon /
        boundary relation)
Closed non-area ways and non-area (e.g. route) relations carry no osmium geometry
and are skipped — rare among dated features and consistent with the project's
prior OHM fidelity. Lines/polygons are simplified (Douglas-Peucker) + rounded; see
SIMPLIFY_TOL (raw admin boundaries reach 12 MB single literals otherwise).

OHM data is CC0 1.0 (public domain). Recommended credit: "Data: OpenHistoricalMap
contributors (CC0)".

Usage:
  python3 scripts/ohm_pbf_to_nt.py PLANET.osm.pbf > data/ohm/ohm-full.nt
Then build (in Docker, the binary is a Linux ELF):
  rete build data/ohm/ohm-full.nt -o data/ohm/ohm-full.rete --no-pyramid
"""
from __future__ import annotations

import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import osmium  # pyosmium >= 4
from shapely import wkt as _shapely_wkt  # geometry simplification (see _simplify)
from ohm_overpass_to_nt import (  # reuse: identical shape + battle-tested date parsing
    EX,
    GEO,
    RDF_TYPE,
    RDFS_LABEL,
    WKT_DT,
    XSD_INT,
    esc,
    years_of,
)

_WKT = osmium.geom.WKTFactory()

# Douglas-Peucker tolerance in degrees applied to ways/areas. Raw OHM admin
# boundaries can be 12 MB single MULTIPOLYGON literals (thousands of vertices) —
# 8.7 GB of the 9.7 GB raw .nt. The atlas/playground render at world/region scale
# and don't need sub-50 m detail, so we simplify + round to 6 decimals (~0.1 m),
# which shrinks the worst boundaries to ~7-8% with shapes intact. Override via
# OHM_SIMPLIFY_TOL (0 disables).
SIMPLIFY_TOL = float(os.environ.get("OHM_SIMPLIFY_TOL", "0.0005"))
_COORD_PRECISION = 6


def _simplify(raw_wkt: str) -> str:
    """Simplify (Douglas-Peucker) + round a line/polygon WKT. Falls back to the
    raw geometry on any parse/simplify error so a feature is never silently lost."""
    try:
        g = _shapely_wkt.loads(raw_wkt)
        if SIMPLIFY_TOL > 0:
            s = g.simplify(SIMPLIFY_TOL, preserve_topology=True)
            if not s.is_empty:
                g = s
        # trim=True drops trailing zeros; strip Shapely's space after the type
        # keyword ("POLYGON (" -> "POLYGON(") so WKT matches the other overlays.
        out = _shapely_wkt.dumps(g, trim=True, rounding_precision=_COORD_PRECISION)
        return re.sub(r"^([A-Z]+) ", r"\1", out, count=1)
    except Exception:  # noqa: BLE001 — keep the feature with its original geometry
        return raw_wkt


def _name(tags):
    n = tags["name"] if "name" in tags else (tags["name:en"] if "name:en" in tags else None)
    return n.strip() if n and n.strip() else None


def _has_min(tags) -> bool:
    """Cheap gate on the live TagList before we build a dict / geometry."""
    return ("start_date" in tags or "start_date:edtf" in tags) and (
        "name" in tags or "name:en" in tags
    )


def _tagdict(tags) -> dict:
    """Copy to a plain dict and fold EDTF date keys onto the plain keys so the
    reused years_of() (which reads start_date/end_date) also sees EDTF-only dates."""
    d = {t.k: t.v for t in tags}
    if "start_date" not in d and "start_date:edtf" in d:
        d["start_date"] = d["start_date:edtf"]
    if "end_date" not in d and "end_date:edtf" in d:
        d["end_date"] = d["end_date:edtf"]
    return d


def _emit(out, iri, name, tagdict, wkt) -> bool:
    sy, ey = years_of(tagdict)
    if sy is None:
        return False
    g = f"{iri}/geom"
    out.write(f"<{iri}> <{RDF_TYPE}> <{EX}OhmFeature> .\n")
    out.write(f'<{iri}> <{RDFS_LABEL}> "{esc(name)}"@en .\n')
    out.write(f'<{iri}> <{EX}startYear> "{sy}"^^<{XSD_INT}> .\n')
    out.write(f'<{iri}> <{EX}endYear> "{ey}"^^<{XSD_INT}> .\n')
    out.write(f"<{iri}> <{GEO}hasGeometry> <{g}> .\n")
    out.write(f'<{g}> <{GEO}asWKT> "{esc(wkt)}"^^<{WKT_DT}> .\n')
    return True


def main() -> None:
    if len(sys.argv) < 2:
        print("usage: ohm_pbf_to_nt.py PLANET.osm.pbf > out.nt", file=sys.stderr)
        sys.exit(2)
    path = sys.argv[1]
    out = sys.stdout
    kept = bad_geom = bad_date = 0

    # with_areas() assembles closed-way areas + multipolygon/boundary relations;
    # with_locations() (flex_mem index) gives ways/areas their node coordinates.
    fp = osmium.FileProcessor(path).with_locations().with_areas()
    for obj in fp:
        try:
            tags = obj.tags
            if not _has_min(tags):
                continue
            name = _name(tags)
            if not name:
                continue
            if isinstance(obj, osmium.osm.Node):
                iri = f"https://www.openhistoricalmap.org/node/{obj.id}"
                wkt = _WKT.create_point(obj)
            elif isinstance(obj, osmium.osm.Way):
                if obj.is_closed():
                    continue  # arrives as an Area (if area-tagged); rare closed lines dropped
                iri = f"https://www.openhistoricalmap.org/way/{obj.id}"
                wkt = _simplify(_WKT.create_linestring(obj))
            elif isinstance(obj, osmium.osm.Area):
                typ = "way" if obj.from_way() else "relation"
                iri = f"https://www.openhistoricalmap.org/{typ}/{obj.orig_id()}"
                wkt = _simplify(_WKT.create_multipolygon(obj))
            else:
                continue  # bare relation (geometry, if any, comes via its Area)
            tagdict = _tagdict(tags)
        except Exception:  # noqa: BLE001 — robust: skip any malformed element/geometry
            bad_geom += 1
            continue
        if not wkt:
            bad_geom += 1
            continue
        if _emit(out, iri, name, tagdict, wkt):
            kept += 1
            if kept % 100000 == 0:
                print(f"ohm-pbf: kept {kept} ...", file=sys.stderr)
        else:
            bad_date += 1

    print(
        f"ohm-pbf: kept {kept} features "
        f"(skipped {bad_date} unparseable-date, {bad_geom} bad/empty-geom)",
        file=sys.stderr,
    )
    print("ohm-pbf: License CC0 1.0 — credit: Data: OpenHistoricalMap contributors (CC0)", file=sys.stderr)


if __name__ == "__main__":
    main()
