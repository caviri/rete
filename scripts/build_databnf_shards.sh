#!/usr/bin/env bash
# Build data.bnf.fr as per-perimeter .rete shards (for federation): each shard
# streams its tar(s) into `rete build - --no-pyramid` so RAM stays bounded (the
# full 716M graph would OOM a 62 GB host). part_NN order = data.gouv.fr resource
# order (confirmed by triple counts). Editions (part_17, 410M) split separately.
set -e
cd /work
B=data/databnf
RB=./target/release/rete
mkdir -p "$B/shards"

build() {  # name  files...
  local name=$1; shift
  echo "=== building databnf-$name from $* ==="
  ( for f in "$@"; do tar -xzOf "$B/$f"; done ) \
    | $RB build - -o "$B/shards/databnf-$name.rete" --no-pyramid --card \
      --title "data.bnf.fr - $name" --license "CC0-1.0" \
      --source "https://data.bnf.fr/" --created "2026-06-27"
  ls -lh "$B/shards/databnf-$name.rete" | awk '{print "  ->", $5, $9}'
}

build persons       part_06.tar.gz part_16.tar.gz part_09.tar.gz   # ~109M (authors + elementary + orgs)
build works         part_01.tar.gz part_21.tar.gz part_10.tar.gz part_02.tar.gz  # ~64M
build rameau        part_05.tar.gz part_04.tar.gz part_18.tar.gz   # ~37M (subjects)
build contributions part_20.tar.gz                                 # ~68M
build misc          part_07.tar.gz part_12.tar.gz part_14.tar.gz part_08.tar.gz \
                    part_15.tar.gz part_03.tar.gz part_11.tar.gz part_13.tar.gz \
                    part_19.tar.gz part_22.tar.gz                   # ~30M (periodicals, places, imslp, codes, vocab)
echo "CORE SHARDS DONE"
