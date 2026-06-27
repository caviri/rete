#!/usr/bin/env bash
# Split the data.bnf.fr Éditions perimeter (part_17, ~410M triples) into ~100M-line
# N-Triples chunks (line splits are safe: 1 triple/line) and build each as its own
# --no-pyramid shard, deleting each chunk after to bound disk. Each chunk (~100M)
# fits 62 GB RAM (persons, 108M, built fine).
set -e
cd /work
B=data/databnf
RB=./target/release/rete
mkdir -p "$B/shards"

echo "=== splitting editions into ~100M-line .nt chunks ==="
tar -xzOf "$B/part_17.tar.gz" | split -l 100000000 -d -a 2 --additional-suffix=.nt - "$B/ed_chunk_"
ls -lh "$B"/ed_chunk_*.nt | awk '{print "  chunk:", $5, $9}'

for c in "$B"/ed_chunk_*.nt; do
  n=$(basename "$c" .nt | sed 's/ed_chunk_//')
  echo "=== building databnf-editions-$n ==="
  $RB build "$c" -o "$B/shards/databnf-editions-$n.rete" --no-pyramid --card \
    --title "data.bnf.fr - editions $n" --license "CC0-1.0" \
    --source "https://data.bnf.fr/" --created "2026-06-27"
  ls -lh "$B/shards/databnf-editions-$n.rete" | awk '{print "  ->", $5, $9}'
  rm -f "$c"   # free disk before the next chunk
done
echo "EDITIONS SHARDS DONE"
