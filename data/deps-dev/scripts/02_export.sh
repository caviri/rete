#!/usr/bin/env bash
# Step 2 — export the NODE/metadata tables for ONE snapshot straight to raw/.
# Queries the partitioned BASE tables with a literal SnapshotAt (cheap: pruned to
# one snapshot). No GCS bucket. Streams to Parquet; resumable per table.
#
# Scope = $TABLES (base table names; see config.env). SNAPSHOT defaults to the
# latest in the Snapshots table; override with SNAPSHOT="2026-07-13 21:01:00...".
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"

echo ">> Node tables: ${TABLES}"
mkdir -p "${RAW_DIR}"
MSYS_NO_PATHCONV=1 docker run --rm \
  -v "${GCLOUD_CONFIG_DIR}:/root/.config/gcloud" \
  -v "${REPO_ROOT}:/w" -w //w \
  -e GCP_PROJECT="${GCP_PROJECT}" \
  -e GOOGLE_CLOUD_QUOTA_PROJECT="${GCP_PROJECT}" \
  -e TABLES="${TABLES}" \
  -e SNAPSHOT="${SNAPSHOT:-}" \
  -e OUT_DIR="/w/data/deps-dev/raw" \
  -e GOOGLE_APPLICATION_CREDENTIALS="/root/.config/gcloud/application_default_credentials.json" \
  python:3.12-slim bash -lc '
    set -e
    pip install --quiet --no-cache-dir \
      "google-cloud-bigquery>=3.25" "google-cloud-bigquery-storage>=2.25" \
      "pyarrow>=17" >/dev/null
    python /w/data/deps-dev/scripts/export_direct.py'

echo
echo ">> Writing SHA256SUMS.txt ..."
( cd "${RAW_DIR}" && find . -name '*.parquet' -print0 \
    | sort -z | xargs -0 sha256sum ) > "${DATASET_DIR}/SHA256SUMS.txt"
echo ">> Done. Next: bash 02b_export_edges.sh for the dependency network."
