"""Emit a JSON-schema companion describing every parquet table of an OpenAIRE
dataset, read straight from the real parquet metadata (schema only, no scan).

For each parquet-<table> dir it records the Arrow field names/types/nullability,
the row count and file count (from _checkpoint.json), and writes one combined
`parquet-schemas.json` at the dataset root. Also drops a per-table
`_schema.json` inside each dir so the companion travels with the table.

Usage:
  python scripts/openaire/emit_schemas.py --root data/openaire/2026 --version 11.1.1
  python scripts/openaire/emit_schemas.py --root data/openaire       --version 3.0
"""

import argparse
import glob
import json
import os

import pyarrow.parquet as pq


def field_type(t):
    return str(t)


def table_schema(pdir):
    files = sorted(glob.glob(os.path.join(pdir, "*.parquet")))
    if not files:
        return None
    md = pq.ParquetFile(files[0]).schema_arrow
    cols = [{"name": f.name, "type": field_type(f.type), "nullable": f.nullable}
            for f in md]
    cp = os.path.join(pdir, "_checkpoint.json")
    rows = files_n = None
    if os.path.exists(cp):
        c = json.load(open(cp))
        rows, files_n = c.get("rows"), c.get("files")
    return {
        "table": os.path.basename(pdir).replace("parquet-", ""),
        "path": os.path.relpath(pdir).replace("\\", "/") + "/*.parquet",
        "rows": rows,
        "parquet_files": files_n if files_n is not None else len(files),
        "columns": cols,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--version", required=True)
    args = ap.parse_args()

    tables = []
    total = 0
    for pdir in sorted(glob.glob(os.path.join(args.root, "parquet-*"))):
        s = table_schema(pdir)
        if s is None:
            continue
        tables.append(s)
        total += s["rows"] or 0
        json.dump(s, open(os.path.join(pdir, "_schema.json"), "w"), indent=1)

    doc = {
        "dataset": "OpenAIRE Graph",
        "version": args.version,
        "source": "https://graph.openaire.eu/docs/data-model",
        "license": "CC-BY-4.0",
        "ontology": "openaire.ttl",
        "total_rows": total,
        "n_tables": len(tables),
        "tables": tables,
    }
    out = os.path.join(args.root, "parquet-schemas.json")
    json.dump(doc, open(out, "w"), indent=1)
    print(f"wrote {out}: {len(tables)} tables, {total:,} rows total")
    for t in tables:
        print(f"  {t['table']:24s} {str(t['rows'] or '?'):>15}  {len(t['columns'])} cols")


if __name__ == "__main__":
    main()
