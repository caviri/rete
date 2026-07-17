"""Convert the Zenodo `records-deleted.csv.gz` exporter files to Parquet.

These list records that have been removed from Zenodo (mostly spam), with the
DOI, parent DOI, removal reason and date — the deletion ledger you reconcile
against the full snapshots (each snapshot is complete, not a delta).

Small enough to do in one pass with Arrow's streaming CSV reader.

Usage:
  python deleted_to_parquet.py            # both site-wide + biosyslit files
"""

import argparse
import gzip
import os

import pyarrow as pa
import pyarrow.csv as pacsv
import pyarrow.parquet as pq

DATA = r"D:\pro\rete\data\zenodo"
JOBS = [
    ("records-deleted-2026-07-10.csv.gz", "records-deleted.parquet"),
    ("biosyslit-records-deleted-2026-03-27.csv.gz", "biosyslit-records-deleted.parquet"),
]

# record_id / parent_id kept as string (they are numeric ids, but treat as ids)
COL_TYPES = {
    "record_id": pa.string(),
    "doi": pa.string(),
    "parent_id": pa.string(),
    "parent_doi": pa.string(),
    "removal_note": pa.string(),
    "removal_reason": pa.string(),
    "removal_date": pa.string(),
    "citation_text": pa.string(),
}


def convert(src, dst):
    with gzip.open(src, "rb") as fh:
        table = pacsv.read_csv(
            fh,
            convert_options=pacsv.ConvertOptions(column_types=COL_TYPES),
        )
    pq.write_table(table, dst, compression="zstd", compression_level=3)
    return table.num_rows


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--data", default=DATA)
    args = ap.parse_args()
    for src_name, dst_name in JOBS:
        src = os.path.join(args.data, src_name)
        dst = os.path.join(args.data, dst_name)
        if not os.path.exists(src):
            print(f"skip (missing): {src_name}")
            continue
        n = convert(src, dst)
        print(f"{dst_name}: {n:,} rows")


if __name__ == "__main__":
    main()
