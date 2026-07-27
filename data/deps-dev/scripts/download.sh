#!/usr/bin/env bash
# deps.dev has NO bulk file dump — the only bulk source is the BigQuery public
# dataset `bigquery-public-data.deps_dev_v1` (CC-BY 4.0), which requires OAuth
# (`gcloud auth application-default login`; an API key will NOT work) and a
# project with a BILLING account (a Sandbox's 10 GB storage cap blocks the big
# tables). Cost stays ~$0: the network scope scans ~842 GB, under the free
# 1 TiB/month query tier.
#
# Ordered runbook (see README.md for the size/cost detail):
#
#   1. cp scripts/config.env.example scripts/config.env   # set GCP_PROJECT
#   2. gcloud auth application-default login              # on the host
#   3. bash scripts/00_auth.sh          # verify ADC reaches BigQuery
#   4. bash scripts/01_probe_sizes.sh   # EXACT per-snapshot sizes + $ (free)
#   5. bash scripts/02_export.sh        # nodes -> raw/
#   6. bash scripts/02b_export_edges.sh # the dependency network -> raw/
set -euo pipefail
sed -n '1,40p' "$(dirname "${BASH_SOURCE[0]}")/../README.md"
echo
echo ">> Start with: bash data/deps-dev/scripts/00_auth.sh (after gcloud login)."
