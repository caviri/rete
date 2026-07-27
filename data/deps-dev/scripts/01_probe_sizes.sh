#!/usr/bin/env bash
# Step 1 — print exact sizes + scan cost BEFORE exporting, via ADC (free).
# Runs probe_snapshot.py: base-table sizes, partitioning, and the per-snapshot
# dry-run scan cost (what each export would bill). Shows why the *Latest views
# are a trap (they scan all ~218 snapshots) and that a literal SnapshotAt prunes.
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"

MSYS_NO_PATHCONV=1 docker run --rm \
  -v "${GCLOUD_CONFIG_DIR}:/root/.config/gcloud" \
  -v "${REPO_ROOT}:/w" -w //w \
  -e GCP_PROJECT="${GCP_PROJECT}" \
  -e GOOGLE_CLOUD_QUOTA_PROJECT="${GCP_PROJECT}" \
  -e GOOGLE_APPLICATION_CREDENTIALS="/root/.config/gcloud/application_default_credentials.json" \
  python:3.12-slim bash -lc '
    pip install --quiet --no-cache-dir "google-cloud-bigquery>=3.25" >/dev/null 2>&1
    python /w/data/deps-dev/scripts/probe_snapshot.py' 2>&1 \
  | grep -v "UserWarning\|warnings.warn"
