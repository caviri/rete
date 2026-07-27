#!/usr/bin/env bash
# Step 2b — the dependency NETWORK. Exports the DIRECT dependency edges of one
# snapshot to raw/dependency_edges.parquet. This single edge set gives BOTH
# directions: "dependencies" = follow edges forward, "dependents" = follow them
# backward. No need for the multi-TB transitive Dependencies (depth>1) or the
# Dependents table — a graph store derives dependents by reverse traversal.
#
# Source: Dependencies base table, one snapshot, MinimumDepth = 1. Pruned by the
# SnapshotAt partition (~709 GB scan for the latest snapshot; under the 1 TiB/mo
# free tier). SNAPSHOT defaults to the latest; override to pin a snapshot.
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"

mkdir -p "${RAW_DIR}"
MSYS_NO_PATHCONV=1 docker run --rm \
  -v "${GCLOUD_CONFIG_DIR}:/root/.config/gcloud" \
  -v "${REPO_ROOT}:/w" -w //w \
  -e GCP_PROJECT="${GCP_PROJECT}" \
  -e GOOGLE_CLOUD_QUOTA_PROJECT="${GCP_PROJECT}" \
  -e EDGES="1" -e TABLES="" \
  -e SNAPSHOT="${SNAPSHOT:-}" \
  -e OUT_DIR="/w/data/deps-dev/raw" \
  -e GOOGLE_APPLICATION_CREDENTIALS="/root/.config/gcloud/application_default_credentials.json" \
  python:3.12-slim bash -lc '
    set -e
    pip install --quiet --no-cache-dir \
      "google-cloud-bigquery>=3.25" "google-cloud-bigquery-storage>=2.25" \
      "pyarrow>=17" >/dev/null
    python /w/data/deps-dev/scripts/export_direct.py'

echo ">> Done. dependents = reverse of these edges (no extra table needed)."
