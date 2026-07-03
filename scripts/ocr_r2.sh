#!/bin/sh
# Robust, resumable OCR: process ONE book at a time. For each book, pull its page
# images from the public R2 CDN into the container's local fs, OCR them in parallel,
# and write that book's hocr as /io/hocr/<control>.tgz (a per-book checkpoint on the
# bind mount). Re-running skips books already done — so a killed container just resumes.
# /io/list.txt holds the relative keys "<control>/<file>".
set -e
BASE="https://data.graphplaza.com/ramon_llull/iiif"
mkdir -p /io/hocr
books=$(sed 's|/.*||' /io/list.txt | sort -u)
nb=$(echo "$books" | wc -l); k=0
for b in $books; do
  k=$((k+1))
  if [ -e "/io/hocr/$b.tgz" ]; then echo "[$k/$nb] skip $b" >&2; continue; fi
  rm -rf /d; mkdir -p "/d/$b"
  grep "^$b/" /io/list.txt \
    | xargs -P 24 -I{} sh -c 'curl -sf --retry 6 --retry-connrefused --retry-delay 1 --max-time 120 -o "/d/{}" "'"$BASE"'/{}" || echo "miss {}" >&2'
  find "/d/$b" -type f \( -name 'p-*.jpg' -o -name 'p-*.jpeg' -o -name 'p-*.png' \) \
    | xargs -P 28 -I{} sh -c 'o="$1"; tesseract "$1" "${o%.*}" hocr -l lat >/dev/null 2>&1 || true' _ {}
  ( cd /d && find "$b" -name '*.hocr' | tar -czf "/io/hocr/$b.tgz" -T - )
  echo "[$k/$nb] $b: $(find /d/$b -name '*.hocr' | wc -l) hocr" >&2
done
rm -rf /d
echo "ALL BOOKS DONE" >&2
