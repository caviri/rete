#!/usr/bin/env bash
# Shared config for the deps.dev export pipeline. Sourced by the numbered scripts.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATASET_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${DATASET_DIR}/../.." && pwd)"
RAW_DIR="${DATASET_DIR}/raw"

if [[ -f "${SCRIPT_DIR}/config.env" ]]; then
  # shellcheck disable=SC1091
  source "${SCRIPT_DIR}/config.env"
else
  echo "!! ${SCRIPT_DIR}/config.env not found — cp config.env.example config.env" >&2
  # shellcheck disable=SC1091
  source "${SCRIPT_DIR}/config.env.example"
fi

: "${GCP_PROJECT:?set GCP_PROJECT in config.env}"
: "${BQ_TMP_DATASET:=deps_dev_export}"
: "${GCLOUD_CONFIG_DIR:=$HOME/.config/gcloud}"
: "${TABLES:=PackageVersions Projects Advisories PackageVersionToProject}"
: "${SNAPSHOT:=}"
