#!/usr/bin/env bash
# Close the loop the other way: mirbase.rete -> Parquet -> FASTA/GFF3, and
# check those against the files miRBase shipped.
#
#   bash data/mirbase/scripts/roundtrip_rete_test.sh
#
# This test is strictly ADDITIVE: it only ever writes into its own scratch
# directories and passes the rete-derived table set to the converters as an
# argument. An earlier version moved data/mirbase/parquet aside and restored it
# from an EXIT trap — which lost the whole directory when the restore did not
# fire. Never mutate the real Parquet layer to run a test.
set -uo pipefail
export MSYS_NO_PATHCONV=1
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
WINREPO="$(cd "$REPO" && { pwd -W 2>/dev/null || pwd; })"
RAW="$HERE/raw"
RT="$HERE/roundtrip-from-rete"
DERIVED="$HERE/parquet-from-rete"
cd "$REPO"

[ -f "$HERE/mirbase.rete" ] || { echo "!! build it first: bash data/mirbase/scripts/build_rete.sh" >&2; exit 1; }
[ -d "$HERE/parquet" ] || { echo "!! data/mirbase/parquet missing — regenerate it first" >&2; exit 1; }
rm -rf "$RT" "$DERIVED"; mkdir -p "$RT/genomes"

echo "==> export .rete -> N-Quads -> Parquet"
docker run --rm -v "$WINREPO:/work" -w /work rete-dev:latest \
  /work/target/release/rete export /work/data/mirbase/mirbase.rete --format nq \
  | docker run -i --rm -v "$WINREPO:/w" -w //w mirbase-py:latest \
      python data/mirbase/scripts/rete_to_parquet.py
[ "${PIPESTATUS[0]}" -eq 0 ] || exit 1

echo
echo "==> Parquet -> FASTA / GFF3, reading the rete-derived tables"
# repo-relative paths: these run INSIDE the container, where the repo is at /w
P_DERIVED="data/mirbase/parquet-from-rete"
bash data/mirbase/scripts/py.sh parquet_to_fa.py \
     data/mirbase/roundtrip-from-rete          "$P_DERIVED" || exit 1
bash data/mirbase/scripts/py.sh parquet_to_gff3.py \
     data/mirbase/roundtrip-from-rete/genomes  "$P_DERIVED" || exit 1

echo
echo "==> compare against the shipped files"
pass=0; fail=0
check() {
  if [ ! -f "$2" ]; then echo "  MISSING  $(basename "$1")"; fail=$((fail+1)); return; fi
  if cmp -s "$1" "$2"; then
    printf '  ok       %-22s %s bytes\n' "$(basename "$1")" "$(wc -c < "$1")"; pass=$((pass+1))
  else
    printf '  DIFFERS  %-22s\n' "$(basename "$1")"; cmp "$1" "$2" | head -1; fail=$((fail+1))
  fi
}
check "$RAW/hairpin.fa" "$RT/hairpin.fa"
check "$RAW/mature.fa"  "$RT/mature.fa"
for g in "$RAW"/genomes/*.gff3; do check "$g" "$RT/genomes/$(basename "$g")"; done

echo
echo "==> $pass passed, $fail failed"
[ "$fail" -eq 0 ] && echo "RETE -> FASTA/GFF3 BYTE-IDENTICAL"
[ "$fail" -eq 0 ]
