"""Convert the ROR (Research Organization Registry) data dump to Parquet.

Input:  data/ror/v1.54-2024-10-21-ror-data.zip  (schema v2 JSON, ~111k orgs)
Output: data/ror/parquet/ror.parquet

One row per organization. Scalars + join keys (ror_id, fundref, grid, isni,
wikidata — these resolve the org_id/funder ids that appear across ORCID /
DataCite / EPFL); nested fields kept as JSON columns.
"""

import os
import zipfile

import orjson
import pyarrow as pa
import pyarrow.parquet as pq

ZIP = r"D:\pro\rete\data\ror\v1.54-2024-10-21-ror-data.zip"
MEMBER = "v1.54-2024-10-21-ror-data_schema_v2.json"
OUT = r"D:\pro\rete\data\ror\parquet"

SCHEMA = pa.schema([
    ("id", pa.string()), ("ror_id", pa.string()), ("name", pa.string()),
    ("status", pa.string()), ("established", pa.int32()),
    ("primary_type", pa.string()),
    ("country_code", pa.string()), ("country_name", pa.string()),
    ("location_name", pa.string()), ("lat", pa.float64()), ("lng", pa.float64()),
    ("geonames_id", pa.int64()),
    ("website", pa.string()), ("wikipedia", pa.string()),
    ("fundref", pa.string()), ("grid", pa.string()), ("isni", pa.string()),
    ("wikidata", pa.string()),
    ("n_relationships", pa.int32()), ("n_names", pa.int32()),
    ("created_date", pa.string()), ("last_modified_date", pa.string()),
    ("types_json", pa.string()), ("names_json", pa.string()),
    ("locations_json", pa.string()), ("links_json", pa.string()),
    ("external_ids_json", pa.string()), ("relationships_json", pa.string()),
    ("domains_json", pa.string()),
])


def j(v):
    return orjson.dumps(v).decode() if v else None


def display_name(names):
    label = None
    for n in names or []:
        ts = n.get("types") or []
        if "ror_display" in ts:
            return n.get("value")
        if label is None and "label" in ts:
            label = n.get("value")
    if label:
        return label
    return names[0].get("value") if names else None


def ext(external_ids, kind):
    for e in external_ids or []:
        if e.get("type") == kind:
            return e.get("preferred") or (e["all"][0] if e.get("all") else None)
    return None


def link(links, kind):
    for l in links or []:
        if l.get("type") == kind:
            return l.get("value")
    return None


def to_int(v):
    try:
        return int(v)
    except (TypeError, ValueError):
        return None


def row(r):
    locs = r.get("locations") or []
    gd = (locs[0].get("geonames_details") or {}) if locs else {}
    return {
        "id": r.get("id"),
        "ror_id": (r.get("id") or "").rsplit("/", 1)[-1] or None,
        "name": display_name(r.get("names")),
        "status": r.get("status"),
        "established": to_int(r.get("established")),
        "primary_type": (r.get("types") or [None])[0],
        "country_code": gd.get("country_code"),
        "country_name": gd.get("country_name"),
        "location_name": gd.get("name"),
        "lat": gd.get("lat"), "lng": gd.get("lng"),
        "geonames_id": to_int(locs[0].get("geonames_id")) if locs else None,
        "website": link(r.get("links"), "website"),
        "wikipedia": link(r.get("links"), "wikipedia"),
        "fundref": ext(r.get("external_ids"), "fundref"),
        "grid": ext(r.get("external_ids"), "grid"),
        "isni": ext(r.get("external_ids"), "isni"),
        "wikidata": ext(r.get("external_ids"), "wikidata"),
        "n_relationships": len(r.get("relationships") or []),
        "n_names": len(r.get("names") or []),
        "created_date": (r.get("admin", {}).get("created") or {}).get("date"),
        "last_modified_date": (r.get("admin", {}).get("last_modified") or {}).get("date"),
        "types_json": j(r.get("types")), "names_json": j(r.get("names")),
        "locations_json": j(r.get("locations")), "links_json": j(r.get("links")),
        "external_ids_json": j(r.get("external_ids")),
        "relationships_json": j(r.get("relationships")),
        "domains_json": j(r.get("domains")),
    }


def main():
    os.makedirs(OUT, exist_ok=True)
    with zipfile.ZipFile(ZIP) as z:
        recs = orjson.loads(z.read(MEMBER))
    print(f"loaded {len(recs):,} ROR records", flush=True)
    cols = {f.name: [] for f in SCHEMA}
    for r in recs:
        d = row(r)
        for k in cols:
            cols[k].append(d[k])
    table = pa.table(cols, schema=SCHEMA)
    pq.write_table(table, os.path.join(OUT, "ror.parquet"),
                   compression="zstd", compression_level=3)
    print(f"wrote {table.num_rows:,} rows -> {OUT}\\ror.parquet "
          f"({os.path.getsize(os.path.join(OUT,'ror.parquet'))/1e6:.1f} MB)", flush=True)


if __name__ == "__main__":
    main()
