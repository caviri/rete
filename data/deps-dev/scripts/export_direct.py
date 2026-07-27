#!/usr/bin/env python3
"""Export ONE deps_dev_v1 snapshot straight to local Parquet — cheap + no bucket.

The *Latest views scan all ~218 snapshots (100s of TB). Instead we query the
partitioned BASE tables with a LITERAL `SnapshotAt = TIMESTAMP(...)`, which prunes
to a single snapshot (~150x cheaper). Each query writes a destination table (large
results can't return inline) that is then read via the BigQuery Storage API and
streamed to Parquet in bounded memory; resumable per table (a *.parquet.done
marker skips it), and the scratch table is dropped afterwards.

Auth: Application Default Credentials (gcloud auth application-default login).

Env:
  GCP_PROJECT   billing/query project (required)
  SNAPSHOT      snapshot timestamp literal; default = latest in Snapshots
  TABLES        space-separated BASE table names to export whole (per snapshot)
  EDGES         if "1", also export raw/dependency_edges.parquet (direct edges)
  OUT_DIR       output root (default /w/data/deps-dev/raw)
"""
import os
import pathlib

import pyarrow as pa
import pyarrow.parquet as pq
from google.cloud import bigquery

DS = "bigquery-public-data.deps_dev_v1"
PROJECT = os.environ["GCP_PROJECT"]
TABLES = os.environ.get("TABLES", "").split()
EDGES = os.environ.get("EDGES", "") == "1"
OUT_DIR = pathlib.Path(os.environ.get("OUT_DIR", "/w/data/deps-dev/raw"))

TMP_DATASET = os.environ.get("BQ_TMP_DATASET", "deps_dev_export")

client = bigquery.Client(project=PROJECT)
OUT_DIR.mkdir(parents=True, exist_ok=True)

# Each query writes to a destination table in this scratch dataset — required for
# large results ("Response too large to return") and lets us read them via the
# Storage API. Created here; the tables are dropped after each export.
_ds = bigquery.Dataset(f"{PROJECT}.{TMP_DATASET}")
_ds.location = "US"
client.create_dataset(_ds, exists_ok=True)

# BigQuery Storage Read API: 10-100x faster than the paginated REST reader for
# bulk pulls, and billed only on RESULT bytes (tens of GB -> cents), not the scan.
# Fall back to REST if the API isn't enabled / permitted.
try:
    from google.cloud import bigquery_storage_v1
    BQS = bigquery_storage_v1.BigQueryReadClient()
    print("using BigQuery Storage Read API (fast)", flush=True)
except Exception as exc:  # noqa: BLE001
    BQS = None
    print(f"Storage API unavailable ({str(exc)[:60]}); REST reader (slow)", flush=True)

SNAPSHOT = os.environ.get("SNAPSHOT", "").strip()
if not SNAPSHOT:
    SNAPSHOT = str(list(client.query(
        f"SELECT MAX(Time) AS s FROM `{DS}`.Snapshots").result())[0].s)
print(f"snapshot = {SNAPSHOT}", flush=True)
TS = f"TIMESTAMP('{SNAPSHOT}')"


def stream(sql: str, name: str) -> None:
    out = OUT_DIR / f"{name}.parquet"
    done = out.with_suffix(".parquet.done")
    if done.exists():
        print(f"== {name}: already complete, skipping", flush=True)
        return
    print(f"== {name}: querying ...", flush=True)
    dest = f"{PROJECT}.{TMP_DATASET}.{name}"
    job = client.query(sql, job_config=bigquery.QueryJobConfig(
        destination=dest, write_disposition="WRITE_TRUNCATE"))
    rows = job.result(page_size=50_000)  # RowIterator over the destination table
    tmp = out.with_suffix(".parquet.part")
    writer, n = None, 0
    try:
        for batch in rows.to_arrow_iterable(bqstorage_client=BQS):
            if writer is None:
                writer = pq.ParquetWriter(tmp, pa.schema(batch.schema),
                                          compression="snappy")
            writer.write_batch(batch)
            n += batch.num_rows
            print(f"   {name}: {n:,} rows", end="\r", flush=True)
    finally:
        if writer is not None:
            writer.close()
    if writer is None:  # zero rows: emit an empty file carrying the schema
        pq.write_table(pa.table([], schema=pa.schema(rows.to_arrow().schema)),
                       tmp, compression="snappy")
    mb = tmp.stat().st_size / 1e6            # size before the move (avoid a race)
    os.replace(tmp, out)                     # atomic, overwrites if present
    done.write_text(f"{n}\n")
    client.delete_table(dest, not_found_ok=True)   # drop the scratch table
    print(f"\n== {name}: {n:,} rows -> {out.name} ({mb:,.1f} MB)", flush=True)


# Node/metadata base tables — whole snapshot.
for tbl in TABLES:
    stream(f"SELECT * FROM `{DS}`.{tbl} WHERE SnapshotAt = {TS}", tbl)

# The dependency NETWORK: direct edges only. Dependents = reverse of these.
if EDGES:
    stream(
        f"""SELECT System, Name AS from_name, Version AS from_version,
                   Dependency.System AS dep_system,
                   Dependency.Name AS to_name, Dependency.Version AS to_version
            FROM `{DS}`.Dependencies
            WHERE SnapshotAt = {TS} AND MinimumDepth = 1""",
        "dependency_edges")

print(f"\nAll exports for snapshot {SNAPSHOT} -> {OUT_DIR}", flush=True)
