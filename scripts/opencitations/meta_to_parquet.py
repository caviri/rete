"""Convert the OpenCitations Meta CSV dataset (Zenodo 20965426, v13.1.0, CC0)
to Parquet.

Input:  data/opencitations/meta-v13.1.0/output_csv_2026_06.7z
        a solid 7z of 45,140 CSVs (~59 GB uncompressed), one shared schema:
        id,title,author,issue,volume,venue,page,pub_date,type,publisher,editor
        where `id` is a space-separated list of PIDs
        (omid: doi: pmid: openalex: issn: isbn: ...).

Output: data/opencitations/meta-v13.1.0/parquet/part-*.parquet
        all 11 raw columns kept, PLUS derived join keys pulled out of `id`:
        omid, doi (lowercased), pmid, openalex, issn, isbn, and pub_year.
        doi/openalex/pmid join to DataCite/OpenAIRE/ORCID/DBLP.

Steps: py7zr extractall (7z can't be read in place by DuckDB) -> DuckDB reads
all CSVs via glob and writes rolling zstd Parquet -> temp CSVs deleted.
DuckDB threads/memory are capped so this coexists with other running jobs.

Usage:
  python scripts/opencitations/meta_to_parquet.py
  python scripts/opencitations/meta_to_parquet.py --keep-tmp --threads 6
"""

import argparse
import os
import shutil
import time

import duckdb
import py7zr

BASE = r"D:\pro\rete\data\opencitations\meta-v13.1.0"
SEVENZ = os.path.join(BASE, "output_csv_2026_06.7z")
TMP = os.path.join(BASE, "_csv_tmp")
OUT = os.path.join(BASE, "parquet")
INNER = "output_csv_2026_06"  # top folder inside the archive


def extract(seven, tmp):
    marker = os.path.join(tmp, "_extract_done")
    if os.path.exists(marker):
        print("extract: already done (marker present), skipping", flush=True)
        return
    os.makedirs(tmp, exist_ok=True)
    print(f"extract: {seven} -> {tmp} ...", flush=True)
    t0 = time.time()
    with py7zr.SevenZipFile(seven, "r") as z:
        z.extractall(path=tmp)
    open(marker, "w").close()
    print(f"extract: done in {(time.time()-t0)/60:.1f} min", flush=True)


def convert(tmp, out, threads, mem):
    csv_glob = os.path.join(tmp, INNER, "*.csv").replace("\\", "/")
    out_u = out.replace("\\", "/")
    os.makedirs(out, exist_ok=True)
    con = duckdb.connect()
    con.execute(f"SET threads={threads}")
    con.execute(f"SET memory_limit='{mem}'")
    con.execute("SET preserve_insertion_order=false")
    t0 = time.time()
    print("convert: DuckDB reading CSVs -> Parquet ...", flush=True)
    con.execute(f"""
        COPY (
          SELECT
            regexp_extract(id, 'omid:(\\S+)', 1)                       AS omid,
            lower(nullif(regexp_extract(id, 'doi:(\\S+)', 1), ''))     AS doi,
            nullif(regexp_extract(id, 'pmid:(\\S+)', 1), '')           AS pmid,
            nullif(regexp_extract(id, 'openalex:(\\S+)', 1), '')       AS openalex,
            nullif(regexp_extract(id, 'issn:(\\S+)', 1), '')           AS issn,
            nullif(regexp_extract(id, 'isbn:(\\S+)', 1), '')           AS isbn,
            TRY_CAST(regexp_extract(pub_date, '([0-9]{{4}})', 1) AS INTEGER) AS pub_year,
            id, title, author, venue, volume, issue, page,
            pub_date, type, publisher, editor
          FROM read_csv('{csv_glob}', header=true, auto_detect=false,
                        delim=',', quote='"', escape='"',
                        null_padding=true, ignore_errors=false,
                        columns={{
                          'id':'VARCHAR','title':'VARCHAR','author':'VARCHAR',
                          'issue':'VARCHAR','volume':'VARCHAR','venue':'VARCHAR',
                          'page':'VARCHAR','pub_date':'VARCHAR','type':'VARCHAR',
                          'publisher':'VARCHAR','editor':'VARCHAR'}})
        ) TO '{out_u}'
        (FORMAT parquet, COMPRESSION zstd, FILE_SIZE_BYTES '1GB',
         FILENAME_PATTERN 'part-{{i}}')
    """)
    n = con.execute(
        f"SELECT count(*) FROM read_parquet('{out_u}/part-*.parquet')"
    ).fetchone()[0]
    print(f"convert: done in {(time.time()-t0)/60:.1f} min, {n:,} rows", flush=True)
    return n


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--7z", dest="seven", default=SEVENZ)
    ap.add_argument("--tmp", default=TMP)
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--threads", type=int, default=6)
    ap.add_argument("--memory", default="8GB")
    ap.add_argument("--keep-tmp", action="store_true")
    args = ap.parse_args()

    extract(args.seven, args.tmp)
    n = convert(args.tmp, args.out, args.threads, args.memory)
    if not args.keep_tmp:
        print("cleanup: deleting extracted CSVs ...", flush=True)
        shutil.rmtree(args.tmp, ignore_errors=True)
    size = sum(os.path.getsize(os.path.join(args.out, f))
               for f in os.listdir(args.out) if f.endswith(".parquet"))
    print(f"DONE: {n:,} rows, {size/1e9:.1f} GB parquet at {args.out}", flush=True)


if __name__ == "__main__":
    main()
