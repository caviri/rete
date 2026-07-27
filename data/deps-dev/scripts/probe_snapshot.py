#!/usr/bin/env python3
"""Decide whether filtering base tables by a LITERAL SnapshotAt prunes the scan.

The *Latest views scan all ~218 snapshots (huge). If the base tables are
partitioned/clustered on SnapshotAt, a literal `WHERE SnapshotAt = TIMESTAMP(..)`
prunes to one snapshot — potentially 200x cheaper. This measures it.
"""
import os

from google.cloud import bigquery

DS = "bigquery-public-data.deps_dev_v1"
PER_TIB = 6.25
client = bigquery.Client(project=os.environ["GCP_PROJECT"])

# Latest snapshot timestamp (Snapshots is tiny; its column is `Time`).
latest = list(client.query(
    f"SELECT MAX(Time) AS s FROM `{DS}`.Snapshots").result())[0].s
print("latest snapshot:", latest)

BASES = ["Advisories", "Projects", "PackageVersions", "PackageVersionToProject",
         "PackageVersionHashes", "Dependencies", "DependencyGraphEdges",
         "Dependents"]

print("\n== partitioning / clustering of base tables ==")
for tbl in BASES:
    ft = client.get_table(f"{DS}.{tbl}")
    tp = ft.time_partitioning
    part = f"time:{tp.field}" if tp else (
        f"range:{ft.range_partitioning.field}" if ft.range_partitioning else "NONE")
    print(f"  {tbl:<26} partition={part:<16} cluster={ft.clustering_fields}")


def dry(sql: str, label: str) -> None:
    cfg = bigquery.QueryJobConfig(dry_run=True, use_query_cache=False)
    try:
        job = client.query(sql, job_config=cfg)
    except Exception as exc:  # noqa: BLE001
        print(f"  {label:<40} ERROR: {str(exc).splitlines()[0][:66]}")
        return
    b = job.total_bytes_processed or 0
    print(f"  {label:<40} scans {b/1e9:>9.2f} GB   ~${(b/2**40)*PER_TIB:>7.2f}")


lit = f"TIMESTAMP('{latest}')"
print(f"\n== dry-run scan with LITERAL SnapshotAt = {lit} ==")
for tbl in BASES:
    dry(f"SELECT * FROM `{DS}`.{tbl} WHERE SnapshotAt = {lit}", f"{tbl} @latest")

print("\n== the NETWORK edge query, one snapshot, direct edges only ==")
dry(f"""SELECT System, Name, Version, Dependency
        FROM `{DS}`.Dependencies
        WHERE SnapshotAt = {lit} AND MinimumDepth = 1""",
    "edges: Dependencies @latest depth=1")

print(f"\nFirst 1 TiB/month free; $ shown is raw at ${PER_TIB}/TiB.")
