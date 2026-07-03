#!/bin/sh
# Container-side OCR: read a tar of page images on stdin, OCR them in parallel on
# the container's LOCAL fs (fast — avoids the slow Windows Docker bind mount),
# write the resulting .hocr files as a tar on stdout. Progress goes to stderr.
set -e
mkdir -p /d && tar -C /d -xf -
cd /d
total=$(find . -type f \( -name 'p-*.jpg' -o -name 'p-*.jpeg' -o -name 'p-*.png' \) | wc -l)
echo "ocr: $total pages" >&2
find . -type f \( -name 'p-*.jpg' -o -name 'p-*.jpeg' -o -name 'p-*.png' \) \
  | xargs -P 28 -I{} sh -c 'o="$1"; tesseract "$1" "${o%.*}" hocr -l lat >/dev/null 2>&1 || true' _ {}
echo "ocr: done, taring hocr back" >&2
find . -name '*.hocr' | tar -cf - -T -
