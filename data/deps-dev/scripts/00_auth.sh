#!/usr/bin/env bash
# Step 0 — authenticate, then verify BigQuery is reachable.
#
# BigQuery needs OAuth (an API key is rejected: "API keys are not supported by
# this API"). The Python client libs (and this pipeline) use Application Default
# Credentials, so the ONE command you need on the host is:
#
#     gcloud auth application-default login
#
# That writes application_default_credentials.json into your gcloud config dir
# (GCLOUD_CONFIG_DIR), which every step mounts into its container. NOTE: the `bq`
# CLI uses a SEPARATE credential store and is NOT used here — everything goes
# through the Python client + ADC. No host gcloud? See the Docker login flow at
# the bottom of this file.
#
# This script verifies ADC reaches BigQuery (non-interactive).
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
    python - <<PY
from google.cloud import bigquery
c = bigquery.Client(project="'"${GCP_PROJECT}"'")
DS = "bigquery-public-data.deps_dev_v1"
snap = list(c.query(f"SELECT MAX(Time) AS s FROM \`{DS}\`.Snapshots").result())[0].s
print("OK — ADC reaches BigQuery. Latest deps.dev snapshot:", snap)
PY'
echo ">> Next: bash data/deps-dev/scripts/01_probe_sizes.sh"

# --- No host gcloud? Log in through the container (run in YOUR terminal, needs a TTY):
#   source data/deps-dev/scripts/config.env
#   MSYS_NO_PATHCONV=1 docker run --rm -it \
#     -v "$GCLOUD_CONFIG_DIR:/root/.config/gcloud" \
#     gcr.io/google.com/cloudsdktool/google-cloud-cli:slim \
#     gcloud auth application-default login --no-launch-browser
