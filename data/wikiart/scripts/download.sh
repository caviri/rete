#!/usr/bin/env bash
# Reproducible harvest of the WikiArt graph from its keyless public JSON API.
#
#   bash data/wikiart/scripts/download.sh          # all phases, in order
#   bash data/wikiart/scripts/download.sh 4        # just phase 4 (resumable)
#
# Every phase is idempotent: re-running skips what is already on disk. Phase 4
# is the long one (~221.5k requests) -- re-run it until it reports 0 remaining.
#
# Python runs in Docker per repo convention; only curl runs on the host.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_DIR="$(cd "${HERE}/.." && pwd)"
RAW_DIR="${DATA_DIR}/raw"
REPO_ROOT="$(cd "${DATA_DIR}/../.." && pwd)"
REL="data/wikiart/scripts"
PHASE="${1:-all}"

mkdir -p "${RAW_DIR}/sitemap"

# Repo convention: everything in Docker. MSYS_NO_PATHCONV + a Windows-style
# mount + a double-slash workdir keeps Git Bash from rewriting /w to W:/.
py() {
  MSYS_NO_PATHCONV=1 docker run --rm \
    -v "$(cd "${REPO_ROOT}" && pwd -W 2>/dev/null || echo "${REPO_ROOT}"):/w" \
    -w //w -e PYTHONIOENCODING=utf-8 -e PYTHONUNBUFFERED=1 \
    -e WIKIART_WORKERS="${WIKIART_WORKERS:-12}" \
    python:3.12-slim python "$@"
}

run() { [ "${PHASE}" = "all" ] || [ "${PHASE}" = "$1" ]; }

# --- phase 0: sitemaps = the authoritative inventory -------------------------
# Used to verify the harvest is complete: 221,558 painting URLs, 5,761 artists.
if run 0; then
  echo "[0/5] sitemaps"
  curl -sSL --fail "https://www.wikiart.org/sitemap/sitemap_index.xml" \
       -o "${RAW_DIR}/sitemap/sitemap_index.xml"
  grep -oE '<loc>[^<]+</loc>' "${RAW_DIR}/sitemap/sitemap_index.xml" \
    | sed 's/<[^>]*>//g' \
    | while read -r u; do
        f="${RAW_DIR}/sitemap/$(basename "$u")"
        [ -s "$f" ] || curl -sSL --fail "$u" -o "$f"
        printf '  %-34s %8s urls\n' "$(basename "$u")" "$(grep -c '<loc>' "$f" || echo 0)"
      done
fi

# --- phase 1: controlled vocabularies ----------------------------------------
# 1a = v2 dictionaries (METERED)   1b = multilingual category labels (unmetered)
if run 1 || run 1a; then echo "[1a/6] dictionaries";        py "${REL}/harvest_dicts.py"; fi
if run 1 || run 1b; then echo "[1b/6] category labels";     py "${REL}/harvest_categories.py"; fi

# --- phase 2: artists (App layer complete; v2 enrichment METERED) ------------
if run 2; then echo "[2/6] artists";      py "${REL}/harvest_artists.py"; fi

# --- phase 3: painting inventory (App layer complete; v2 ids METERED) --------
if run 3; then echo "[3/6] painting index"; py "${REL}/harvest_painting_index.py"; fi

# --- phase 4: painting detail (the long ones, ~221.5k requests each) ---------
# 4a = v2 layer (description, tags[], galleries[]) for paintings with a Mongo id
# 4b = App layer (dictionaries[], auction/price, technique) for ALL paintings
if run 4 || run 4a; then echo "[4a/6] painting detail (v2)";  py "${REL}/harvest_paintings.py"; fi
if run 4 || run 4b; then echo "[4b/6] painting detail (App)"; py "${REL}/harvest_imagejson.py"; fi

# --- phase 5: image URL manifest ---------------------------------------------
if run 5; then echo "[5/6] image manifest"; py "${REL}/extract_image_urls.py"; fi

# --- phase 6: profile --------------------------------------------------------
if run 6; then echo "[6/7] profile"; py "${REL}/inspect.py"; fi

# --- phase 7: mirror images as 1200px WebP (OPT-IN: hours, ~76 GB of traffic) -
# Not part of `all` -- run it explicitly. Needs Pillow, so it uses its own image.
if [ "${PHASE}" = "7" ]; then
  echo "[7/7] image mirror (WebP)"
  docker image inspect wikiart-webp:latest >/dev/null 2>&1 || {
    echo "  building wikiart-webp:latest (python:3.12-slim + pillow)"
    printf 'FROM python:3.12-slim\nRUN pip install --no-cache-dir pillow\n' \
      | docker build -q -t wikiart-webp:latest -f - . >/dev/null
  }
  MSYS_NO_PATHCONV=1 docker run --rm \
    -v "$(cd "${REPO_ROOT}" && pwd -W 2>/dev/null || echo "${REPO_ROOT}"):/w" \
    -w //w -e PYTHONIOENCODING=utf-8 -e PYTHONUNBUFFERED=1 \
    -e WIKIART_IMG_WORKERS="${WIKIART_IMG_WORKERS:-24}" \
    -e WIKIART_WEBP_Q="${WIKIART_WEBP_Q:-80}" \
    -e WIKIART_MAX_EDGE="${WIKIART_MAX_EDGE:-1200}" \
    wikiart-webp:latest python "${REL}/mirror_images_webp.py"
fi

# --- checksums ---------------------------------------------------------------
if [ "${PHASE}" = "all" ]; then
  ( cd "${RAW_DIR}" && find . -maxdepth 2 -type f \
      \( -name '*.jsonl' -o -name '*.json' -o -name '*.xml' \) ! -name '.*' \
      -print0 | sort -z | xargs -0 sha256sum > "${DATA_DIR}/SHA256SUMS.txt" )
  echo "wrote ${DATA_DIR}/SHA256SUMS.txt"
fi
