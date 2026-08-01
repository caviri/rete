#!/usr/bin/env bash
# Social-preview pipeline, in Docker (the Playwright image, same one the gate uses).
#
#   scripts/preview/run.sh capture [--scope=all] [--dataset=x] [--concurrency=4]
#   scripts/preview/run.sh finalize          # cache JSONL -> web/preview/answers.json
#   scripts/preview/run.sh inject            # social tags into the pre-built app pages
#   scripts/preview/run.sh cards             # answers.json -> docs/og/*.png
#   scripts/preview/run.sh pages             # answers.json -> docs/q/*.html + docs/d/*.html
#   scripts/preview/run.sh build             # inject + cards + pages (no re-capture)
#   scripts/preview/run.sh all               # capture + build
#
# `cargo run -p docgen` must have run first: the docs cards are rendered from the
# social tags docgen writes into docs/*.html.
#
# Browsers come from the image; the only npm dependency is playwright itself,
# installed once into tests/gate/node_modules (gitignored).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE="mcr.microsoft.com/playwright:v1.49.0-jammy"
CMD="${1:-all}"
shift || true

run_in_docker() {
  MSYS_NO_PATHCONV=1 docker run --rm \
    -v "${ROOT}:/work" -w /work \
    --shm-size=1g \
    "${IMAGE}" bash -lc "$1"
}

# pngquant takes the flat cards from ~240 KB to ~40 KB with no visible loss,
# which matters when ~800 of them are committed. The image does not ship it, so
# install it into the ephemeral container before rendering (a few seconds).
PNGQUANT_SETUP='(command -v pngquant >/dev/null || (apt-get update -qq && apt-get install -y -qq pngquant >/dev/null 2>&1)) || true'

ensure_deps() {
  if [ ! -d "${ROOT}/tests/gate/node_modules/playwright" ]; then
    echo "── installing playwright into tests/gate/node_modules ──"
    run_in_docker "cd /work/tests/gate && npm ci --no-audit --no-fund"
  fi
}

# The captured result thumbnails are committed (they are an input to the card
# render), and Chromium writes them as full-colour PNGs — ~140 KB each for what
# ends up as a 700px-wide panel. Quantizing them costs nothing visible and takes
# the directory from ~10 MB to ~2 MB.
#
# Only truecolour files are touched. Byte 25 of a PNG is the IHDR colour type,
# and 3 means "already palette-indexed": re-quantizing those is not quite a
# no-op, so without this guard every build would rewrite a few thumbnails,
# re-render the cards that embed them, and lose a little more quality each time.
SHOTS_OPT='for f in /work/web/preview/shots/*.png; do [ -e "$f" ] || continue; [ "$(od -An -tu1 -j25 -N1 "$f" | tr -d " ")" = "3" ] && continue; pngquant --force --skip-if-larger --quality=60-90 --speed 1 --strip --ext .png "$f" || true; done'

build_all() {
  run_in_docker "node /work/scripts/preview/inject_og.mjs"
  run_in_docker "${PNGQUANT_SETUP}; ${SHOTS_OPT}; node /work/scripts/preview/render_cards.mjs $*"
  run_in_docker "node /work/scripts/preview/build_pages.mjs"
}

case "${CMD}" in
  capture)  ensure_deps; run_in_docker "node /work/scripts/preview/capture.mjs $*" ;;
  finalize) ensure_deps; run_in_docker "node /work/scripts/preview/capture.mjs --finalize" ;;
  inject)   ensure_deps; run_in_docker "node /work/scripts/preview/inject_og.mjs $*" ;;
  cards)    ensure_deps; run_in_docker "${PNGQUANT_SETUP}; node /work/scripts/preview/render_cards.mjs $*" ;;
  pages)    ensure_deps; run_in_docker "node /work/scripts/preview/build_pages.mjs $*" ;;
  check)    ensure_deps; run_in_docker "node /work/tests/gate/checks/check_social_previews.mjs" ;;
  build)    ensure_deps; build_all "$@" ;;
  all)
    ensure_deps
    run_in_docker "node /work/scripts/preview/capture.mjs $*"
    build_all
    ;;
  *) echo "usage: $0 {capture|finalize|inject|cards|pages|build|check|all} [flags]" >&2; exit 2 ;;
esac
