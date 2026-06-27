#!/usr/bin/env bash
# Build Biblissima+ (~254M triples, harvested per-entity RDF) as federated shards:
# the monolithic build OOMs a 62 GB host (Wikibase RDF = huge multilingual-label
# dictionary), so split the decompressed N-Triples into 90M-line chunks (< the 108M
# persons shard that built fine) and build each --no-pyramid. Chunks deleted as we go.
set -e
cd /work
B=data/biblissima
RB=./target/release/rete
mkdir -p "$B/rete"

echo "=== splitting ~254M triples into 90M-line .nt chunks ==="
zcat "$B"/shards/*.nt.gz | split -l 90000000 -d -a 2 --additional-suffix=.nt - "$B/bib_chunk_"
ls -lh "$B"/bib_chunk_*.nt | awk '{print "  chunk:", $5, $9}'

for c in "$B"/bib_chunk_*.nt; do
  n=$(basename "$c" .nt | sed 's/bib_chunk_//')
  echo "=== building biblissima-$n ==="
  $RB build "$c" -o "$B/rete/biblissima-$n.rete" --no-pyramid --card \
    --title "Biblissima+ $n" \
    --source "https://data.biblissima.fr/" --created "2026-06-27"
  ls -lh "$B/rete/biblissima-$n.rete" | awk '{print "  ->", $5, $9}'
  rm -f "$c"
done
echo "BIBLISSIMA SHARDS DONE"
