#!/usr/bin/env bash
# Prove the converters are lossless: raw -> Parquet -> raw must be byte-identical
# to the files miRBase shipped, for FASTA, EMBL and GFF3.
#
#   bash data/mirbase/scripts/roundtrip_test.sh
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
RAW="$HERE/raw"
RT="$HERE/roundtrip"
cd "$REPO"

rm -rf "$RT"; mkdir -p "$RT"

echo "==> forward:  raw -> parquet"
bash data/mirbase/scripts/py.sh fa_to_parquet.py    || exit 1
bash data/mirbase/scripts/py.sh gff3_to_parquet.py  || exit 1
bash data/mirbase/scripts/py.sh embl_to_parquet.py  || exit 1

echo
echo "==> reverse:  parquet -> raw"
bash data/mirbase/scripts/py.sh parquet_to_fa.py    || exit 1
bash data/mirbase/scripts/py.sh parquet_to_gff3.py  || exit 1
bash data/mirbase/scripts/py.sh parquet_to_embl.py  || exit 1

echo
echo "==> compare (byte-for-byte)"
fail=0; pass=0
check() { # check <original> <rebuilt>
  if [ ! -f "$2" ]; then echo "  MISSING  $2"; fail=$((fail+1)); return; fi
  if cmp -s "$1" "$2"; then
    printf '  ok       %-26s %s bytes\n' "$(basename "$1")" "$(wc -c < "$1")"
    pass=$((pass+1))
  else
    printf '  DIFFERS  %-26s\n' "$(basename "$1")"
    cmp "$1" "$2" | head -2
    fail=$((fail+1))
  fi
}

for f in hairpin.fa hairpin_high_conf.fa mature.fa mature_high_conf.fa \
         miRNA.dat miRNA_high_conf.dat; do
  check "$RAW/$f" "$RT/$f"
done
for g in "$RAW"/genomes/*.gff3; do
  check "$g" "$RT/genomes/$(basename "$g")"
done

echo
echo "==> $pass passed, $fail failed"
[ "$fail" -eq 0 ] || exit 1
echo "ALL ROUND-TRIPS BYTE-IDENTICAL"
