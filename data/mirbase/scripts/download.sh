#!/usr/bin/env bash
# miRBase release 22.1 — full download.
#
#   https://www.mirbase.org/   (public domain — see raw/LICENSE)
#
# THE ONE GOTCHA THAT MATTERS (see README "Gotchas"):
#   miRBase's Django site serves the SAME file two different ways.
#     /download/<file>            -> raw bytes            (only for SOME files)
#     /download/CURRENT/<file>    -> HTML-WRAPPED payload (<p>, <br>, &gt; ...)
#   Everything under database_files/ and the *_high_conf / .str / README /
#   LICENSE files exist ONLY in the wrapped form, so they are downloaded as-is
#   into raw/_wrapped_html/ and then losslessly un-wrapped by unwrap_html.py.
#   The un-wrap is verified byte-for-byte against hairpin.fa, which the server
#   happens to serve BOTH ways.
#
# Usage:  bash data/mirbase/scripts/download.sh
set -euo pipefail

BASE="https://www.mirbase.org/download"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RAW="$HERE/raw"
WRAP="$RAW/_wrapped_html"
REPO="$(cd "$HERE/../.." && pwd)"

mkdir -p "$RAW/genomes" "$RAW/database_files" "$WRAP/database_files"

# curl with retries; --fail so a 404 HTML page never lands as data.
fetch() { # fetch <url> <dest>
  curl -sSL --fail --retry 5 --retry-delay 2 --retry-all-errors \
       --connect-timeout 20 --max-time 900 "$1" -o "$2"
}

echo "==> [1/4] root files served RAW at /download/<file>"
for f in hairpin.fa mature.fa miRNA.dat miRNA.dead miRNA.diff; do
  echo "    $f"; fetch "$BASE/$f" "$RAW/$f"
done
# The site's "miRNA.xls" link is really a CSV, and it carries a UTF-8 BOM.
echo "    miRNA.csv"; fetch "$BASE/miRNA.csv" "$RAW/miRNA.csv"

echo "==> [2/4] 31 genome-coordinate GFF3 files (served RAW)"
SPECIES="aae ame ath bmo bta cbr cel cfa cre dme dps dre ebv fru gga hcmv hsa \
kshv mdo mghv mml mmu osa ptc ptr rno sme tni vvi xtr zma"
for s in $SPECIES; do
  printf '    %s.gff3\n' "$s"; fetch "$BASE/$s.gff3" "$RAW/genomes/$s.gff3"
done

echo "==> [3/4] files that exist ONLY HTML-wrapped under /download/CURRENT/"
for f in README LICENSE hairpin_high_conf.fa mature_high_conf.fa \
         miRNA_high_conf.dat miRNA.str; do
  echo "    $f"; fetch "$BASE/CURRENT/$f" "$WRAP/$f"
done
# Wrapped copy of a file we ALSO have raw — the un-wrap self-test fixture.
fetch "$BASE/CURRENT/hairpin.fa" "$WRAP/hairpin.fa.wrapped"

# The 18-table relational dump — the real graph spine.
TABLES="confidence confidence_score dead_mirna literature_references \
mature_database_links mature_database_url mirna mirna_2_prefam \
mirna_chromosome_build mirna_context mirna_database_links mirna_database_url \
mirna_literature_references mirna_mature mirna_pre_mature mirna_prefam \
mirna_species"
for t in $TABLES; do
  printf '    database_files/%s.txt\n' "$t"
  fetch "$BASE/CURRENT/database_files/$t.txt" "$WRAP/database_files/$t.txt"
done
echo "    database_files/tables.sql"
fetch "$BASE/CURRENT/database_files/tables.sql" "$WRAP/database_files/tables.sql"

echo "==> [4/4] un-wrapping (Docker; self-tests against raw hairpin.fa)"
MSYS_NO_PATHCONV=1 docker run --rm -v "$REPO:/w" -w //w python:3.12-slim \
  python data/mirbase/scripts/unwrap_html.py

echo "==> checksums"
cd "$RAW"
find . -type f ! -path './_wrapped_html/*' -print0 \
  | sort -z | xargs -0 sha256sum > "$HERE/SHA256SUMS.txt"
echo "wrote $HERE/SHA256SUMS.txt ($(wc -l < "$HERE/SHA256SUMS.txt") files)"
du -sh "$RAW"
