# deps-dev

Raw snapshot of **deps.dev (Open Source Insights)** — Google's cross-ecosystem
graph of open-source packages, versions, projects, licenses, advisories, and
dependency relations (npm, Go, Maven, PyPI, Cargo, NuGet, RubyGems).

- Source: <https://deps.dev/> · docs <https://docs.deps.dev/bigquery/v1/>
- **License: CC-BY 4.0** — attribution: "Includes data from deps.dev (Open Source
  Insights) by Google, licensed under CC BY 4.0."
- Bulk access: **BigQuery public dataset only** — `bigquery-public-data.deps_dev_v1`.
  No file dump, no public GCS bucket. Access needs **OAuth** (`gcloud auth
  application-default login`, or a service account); a Google **API key is
  rejected** (`API keys are not supported by this API`).
- Snapshot exported: **`2026-07-13 21:01:00 UTC`** (the latest at build time; the
  base tables retain all ~218 historical snapshots — pick one via `SnapshotAt`).
- Downloaded: **2026-07-24** — 5 Parquet files, ~87 GB. Row counts:

  | File | Rows | Size |
  |---|---|---|
  | PackageVersions.parquet | 161,888,666 | 64.21 GB |
  | dependency_edges.parquet | 570,601,975 | 17.03 GB |
  | PackageVersionToProject.parquet | 172,093,192 | 5.29 GB |
  | Projects.parquet | 5,122,936 | 0.31 GB |
  | Advisories.parquet | 272,582 | 0.07 GB |

  Backed up to R2 (range-readable), public at
  `https://data.graphplaza.com/deps-dev/raw/<file>.parquet` (+ `_parquet_manifest.json`).
  Re-upload with `data/deps-dev/scripts/upload_r2.py` (resumable, skips same-size).

  **Integrity:** all five files pass a full row-group decode (every row group
  reads). One caution learned here: Docker's Windows bind-mount silently corrupted
  ~1,700 row groups of the first `Projects.parquet` under heavy concurrent writes —
  the file's size and SHA still "matched" (the corruption is intra-file, not
  truncation), so **only a full Parquet decode catches it**, not size/checksum.
  Verify with `pyarrow`: `ParquetFile(p).read_row_group(i)` for every `i` (or scan
  with DuckDB). The corrupt Projects was re-pulled and re-verified clean.

## The key gotcha: query the BASE tables by a literal `SnapshotAt`

Every base table is **partitioned by `SnapshotAt`**. The `*Latest` views look
convenient but **defeat partition pruning** — a `SELECT *` on `PackageVersionsLatest`
scans **all 218 snapshots (~11 TB, ~$65)**, and the edge query on `DependenciesLatest`
scans **~102 TB (~$580)**. Filtering a base table with a **literal**
`WHERE SnapshotAt = TIMESTAMP('2026-07-13 …')` prunes to **one** snapshot —
~150× cheaper. That's what the exporter does.

## Size + cost (measured, one snapshot)

`01_probe_sizes.sh` prints these from a free dry-run. Per-snapshot scan (billed at
$6.25/TiB, **first 1 TiB/month free**):

| Table (one snapshot) | Scan | Cost | Role |
|---|---|---|---|
| Advisories | 0.26 GB | ~$0 | node enrichment |
| Projects | 0.60 GB | ~$0 | node enrichment |
| PackageVersionToProject | 22 GB | ~$0.12 | package↔project links |
| PackageVersions | 110 GB | ~$0.62 | **nodes** (licenses, advisories, SLSA) |
| Dependencies @ depth=1 | 709 GB | ~$4.03 | **the network edges** |

**Network scope total ≈ 842 GB scanned — under the free 1 TiB/month → ~$0.**
Parquet-on-disk is far smaller than scan size (Advisories 110 GB→? no: 0.26 GB
scan → 66 MB file; Projects 0.6 GB → 307 MB). The full network lands in a few GB
to low tens of GB, well within this disk.

> **Storage caveat:** large results must transit a destination table. A **BigQuery
> Sandbox** (no billing) caps project storage at **10 GB**, so PackageVersions and
> the edges fail there with `Quota exceeded: free storage`. Attach a **billing
> account** — cost still ~$0 within the free query tier; the scratch tables are
> dropped immediately.

## Layout

```
data/deps-dev/
  README.md
  SHA256SUMS.txt           # filled by the export
  raw/
    Advisories.parquet         Projects.parquet
    PackageVersions.parquet    PackageVersionToProject.parquet
    dependency_edges.parquet   # the direct-dependency network
    *.parquet.done             # per-table completion markers (resume)
  scripts/
    config.env.example     # copy to config.env; set GCP_PROJECT + scope
    _lib.sh                # Dockerized helpers (ADC mounted)
    download.sh            # entrypoint / ordered runbook
    00_auth.sh             # verify ADC reaches BigQuery (Python, not bq CLI)
    01_probe_sizes.sh      # exact per-snapshot sizes + $ (free dry-run)
    probe_snapshot.py      #   (the probe it runs)
    02_export.sh           # nodes -> raw/ (base tables, one snapshot, no bucket)
    02b_export_edges.sh    # the NETWORK: direct dependency edges -> raw/
    export_direct.py       #   (the streaming exporter both wrappers run)
    profile_parquet.py     # profile the downloaded Parquet
    90_export_gcs.sh       # OPTIONAL heavy path: extract -> your GCS bucket
    91_download_gcs.sh     # OPTIONAL: pull GCS Parquet -> raw/
```

## The dependency + dependents network

One directed graph: `packageVersion --dependsOn--> packageVersion`.

- **Edges:** `Dependencies WHERE SnapshotAt = <latest> AND MinimumDepth = 1` — the
  *direct* dependencies (each row = one version depending directly on another).
  Avoids the flattened transitive closure (`MinimumDepth > 1`) and the per-root
  explosion of `DependencyGraphEdges`. Produced by `02b_export_edges.sh`.
- **Dependents come free.** "Dependents of B" = the *reverse* of these edges — in
  a `.rete`/SPARQL graph just query `?x dependsOn B`. So the separate `Dependents`
  table and the multi-TB transitive `Dependencies` are **not needed**.
- **Nodes:** `PackageVersions` (licenses, advisories, VersionInfo, SLSA, Purl),
  enriched with `Projects` + `Advisories`. Produced by `02_export.sh`.

## Reproduce

```bash
cd data/deps-dev/scripts
cp config.env.example config.env         # set GCP_PROJECT (+ scope in TABLES)

# one-time OAuth on the host (API keys do NOT work); needs a billing account:
gcloud auth application-default login

bash 00_auth.sh                # confirm ADC reaches BigQuery
bash 01_probe_sizes.sh         # EXACT per-snapshot sizes + cost before pulling
bash 02_export.sh              # nodes -> raw/
bash 02b_export_edges.sh       # the dependency network -> raw/

# profile:
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD/../../..:/w" -w //w \
  -e OUT_DIR=/w/data/deps-dev/raw python:3.12-slim \
  bash -lc 'pip install -q pyarrow && python /w/data/deps-dev/scripts/profile_parquet.py'
```

## Next step

Groundwork for a `deps-dev.rete`: `PackageVersions` as nodes,
`dependency_edges.parquet` as the edge set (dependents via reverse traversal),
`Advisories`/`Projects` as enrichment. Hand off to the **rete-from-graph** skill
once `raw/` is populated.
