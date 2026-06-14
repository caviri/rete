#!/usr/bin/env python3
"""OHM planet .osm.pbf -> line-delimited GeoJSON (GeoJSONSeq) for Tippecanoe.

Same feature set as ohm_pbf_to_nt.py (named + dated + geometried) so the PMTiles
layer and the .rete graph cover the SAME features and join 1:1 by IRI — but emits
RAW geometry (NO simplification; Tippecanoe does per-zoom LOD itself) as one GeoJSON
Feature per line. Properties: ohm ("type/id", joins to the .rete IRI
openhistoricalmap.org/<ohm>), name, start, end (signed year, 2100 = still present).

Usage: python3 scripts/ohm_pbf_to_geojson.py PLANET.osm.pbf > data/ohm/ohm.geojsonl
Then:  tippecanoe -o ohm.pmtiles -Z0 -z14 --drop-densest-as-needed ... ohm.geojsonl
"""
from __future__ import annotations

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import osmium
from ohm_overpass_to_nt import years_of

_GJ = osmium.geom.GeoJSONFactory()


def _name(tags):
    n = tags["name"] if "name" in tags else (tags["name:en"] if "name:en" in tags else None)
    return n.strip() if n and n.strip() else None


def _has_min(tags):
    return ("start_date" in tags or "start_date:edtf" in tags) and (
        "name" in tags or "name:en" in tags
    )


def _tagdict(tags):
    d = {t.k: t.v for t in tags}
    if "start_date" not in d and "start_date:edtf" in d:
        d["start_date"] = d["start_date:edtf"]
    if "end_date" not in d and "end_date:edtf" in d:
        d["end_date"] = d["end_date:edtf"]
    return d


def main():
    if len(sys.argv) < 2:
        print("usage: ohm_pbf_to_geojson.py PLANET.osm.pbf > out.geojsonl", file=sys.stderr)
        sys.exit(2)
    out = sys.stdout
    kept = bad = 0
    fp = osmium.FileProcessor(sys.argv[1]).with_locations().with_areas()
    for obj in fp:
        try:
            tags = obj.tags
            if not _has_min(tags):
                continue
            name = _name(tags)
            if not name:
                continue
            if isinstance(obj, osmium.osm.Node):
                typ, oid, gj = "node", obj.id, _GJ.create_point(obj)
            elif isinstance(obj, osmium.osm.Way):
                if obj.is_closed():
                    continue
                typ, oid, gj = "way", obj.id, _GJ.create_linestring(obj)
            elif isinstance(obj, osmium.osm.Area):
                typ, oid = ("way" if obj.from_way() else "relation"), obj.orig_id()
                gj = _GJ.create_multipolygon(obj)
            else:
                continue
            sy, ey = years_of(_tagdict(tags))
            if sy is None:
                continue
            geom = json.loads(gj)
        except Exception:  # noqa: BLE001 — robust: skip malformed element/geometry
            bad += 1
            continue
        feat = {
            "type": "Feature",
            "properties": {"ohm": f"{typ}/{oid}", "name": name, "start": sy, "end": ey},
            "geometry": geom,
        }
        out.write(json.dumps(feat, ensure_ascii=False))
        out.write("\n")
        kept += 1
        if kept % 100000 == 0:
            print(f"ohm-geojson: {kept} ...", file=sys.stderr)
    print(f"ohm-geojson: wrote {kept} features ({bad} skipped)", file=sys.stderr)


if __name__ == "__main__":
    main()
