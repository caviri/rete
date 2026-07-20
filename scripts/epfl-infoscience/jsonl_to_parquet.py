"""Convert the harvested EPFL Infoscience JSONL (one file per entity type) into
Parquet — one parquet-<entitytype>/ dir per type.

Reads data/epfl-infoscience/jsonl/<type>.jsonl (as written by harvest_rest.py):
each line is {uuid, handle, name, entityType, lastModified, in/discover/withdrawn,
doi, orcid, sciper, metadata}. The full DSpace-CRIS metadata map is kept whole as
a JSON-string column (`metadata_json`, lossless — incl. the `authority` relation
links) and the analytically useful scalars are typed columns.

Usage:
  python scripts/epfl-infoscience/jsonl_to_parquet.py                 # all jsonl files
  python scripts/epfl-infoscience/jsonl_to_parquet.py --types person,journal
"""

import argparse
import glob
import json
import os

import pyarrow as pa
import pyarrow.parquet as pq

INDIR = r"D:\pro\rete\data\epfl-infoscience\jsonl"
OUTBASE = r"D:\pro\rete\data\epfl-infoscience"

SCHEMA = pa.schema([
    ("uuid", pa.string()),
    ("handle", pa.string()),
    ("name", pa.string()),
    ("entity_type", pa.string()),
    ("last_modified", pa.string()),
    ("in_archive", pa.bool_()),
    ("discoverable", pa.bool_()),
    ("withdrawn", pa.bool_()),
    ("doi", pa.string()),
    ("orcid", pa.string()),
    ("sciper", pa.string()),
    ("metadata_json", pa.string()),
])
# the fulltext table has its own shape (uuid, handle, doi, name, counts, text)
FULLTEXT_SCHEMA = pa.schema([
    ("uuid", pa.string()), ("handle", pa.string()), ("doi", pa.string()),
    ("name", pa.string()), ("n_text_bitstreams", pa.int32()),
    ("n_chars", pa.int64()), ("text", pa.string()),
])
BATCH = 20_000


def row(rec):
    return {
        "uuid": rec.get("uuid"),
        "handle": rec.get("handle"),
        "name": rec.get("name"),
        "entity_type": rec.get("entityType"),
        "last_modified": rec.get("lastModified"),
        "in_archive": rec.get("inArchive"),
        "discoverable": rec.get("discoverable"),
        "withdrawn": rec.get("withdrawn"),
        "doi": rec.get("doi"),
        "orcid": rec.get("orcid"),
        "sciper": rec.get("sciper"),
        "metadata_json": json.dumps(rec.get("metadata"), ensure_ascii=False)
                         if rec.get("metadata") else None,
    }


def ft_row(rec):
    return {"uuid": rec.get("uuid"), "handle": rec.get("handle"), "doi": rec.get("doi"),
            "name": rec.get("name"), "n_text_bitstreams": rec.get("n_text_bitstreams"),
            "n_chars": rec.get("n_chars"), "text": rec.get("text")}


def convert(jsonl_path, out_dir, schema=SCHEMA, rowfn=row):
    os.makedirs(out_dir, exist_ok=True)
    writer = pq.ParquetWriter(os.path.join(out_dir, "part-00000.parquet"),
                              schema, compression="zstd", compression_level=3)
    cols = {f.name: [] for f in schema}
    key = schema.names[0]
    n = 0

    def flush():
        if not cols[key]:
            return
        writer.write_table(pa.table(cols, schema=schema))
        for k in cols:
            cols[k] = []

    with open(jsonl_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            r = rowfn(json.loads(line))
            for k, v in r.items():
                cols[k].append(v)
            n += 1
            if len(cols[key]) >= BATCH:
                flush()
    flush()
    writer.close()
    return n


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--in-dir", default=INDIR)
    ap.add_argument("--out-base", default=OUTBASE)
    ap.add_argument("--types", default="", help="comma-separated entity types (default: all jsonl found)")
    args = ap.parse_args()

    if args.types:
        files = [os.path.join(args.in_dir, f"{t.strip().lower()}.jsonl")
                 for t in args.types.split(",") if t.strip()]
    else:
        files = sorted(glob.glob(os.path.join(args.in_dir, "*.jsonl")))
    grand = 0
    for jf in files:
        if not os.path.exists(jf):
            print(f"skip (missing): {jf}")
            continue
        etype = os.path.basename(jf)[:-6]
        out_dir = os.path.join(args.out_base, f"parquet-{etype}")
        if etype == "fulltext":
            n = convert(jf, out_dir, schema=FULLTEXT_SCHEMA, rowfn=ft_row)
        else:
            n = convert(jf, out_dir)
        grand += n
        print(f"{etype:14s} {n:>8,} rows -> {out_dir}")
    print(f"TOTAL {grand:,} rows")


if __name__ == "__main__":
    main()
