#!/usr/bin/env bash
# Swiss federal law (Fedlex) -> ONE range-queryable .rete, streamed.
#
# The harvest is 100 gzipped N-Quads shards (raw/quads/part-*.nq.gz, 66,392,663
# quads). They are decompressed to ONE .nq on disk (17.3 GB) and built from that
# file — see the note on the build path below.
#
# `--format nq` is load-bearing: this graph is ~498k NAMED GRAPHS (one per act,
# version and language) and that structure IS the dataset — flattening it would
# throw away the per-act provenance Fedlex is built around.
#
# Named graphs also rule out `--memory-budget-mb`: the external build is
# DEFAULT-GRAPH ONLY and stops with "named graph … found". The low-RAM path
# that DOES handle quads is the FILE one — given .nq files rather than stdin,
# `rete build` streams the inputs TWICE instead of materializing the quad
# multiset, which is the difference between a ~44 GB and a ~6 GB build on an
# 88 M-triple graph. Hence the decompress-to-disk step.
#
# `--no-pyramid` for the same reason: the pyramid summarizes the DEFAULT graph,
# which here is nearly empty, so it would cost build time to produce nothing (the
# trap biomni already walked into). SPARQL, SHACL, triple and reachability queries
# do not use it; only community/summary/progressive do.
#
# The JOLux + ELI TBox needs no separate input — the endpoint serves its own
# ontology graphs, so they came through the harvest (720k jolux triples in shard 0
# alone).
set -uo pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"
OUT="${OUT:-data/switzerland-fedlex/switzerland-fedlex.rete}"
BUDGET="${RETE_BUDGET_MB:-2500}"

echo "=== fedlex build: 66,392,663 quads from 100 shards, budget ${BUDGET} MiB ===" >&2
S=$(date +%s)

docker run --rm --user root -v "$ROOT:/work" -w //work rete-dev:latest bash -c "
    cd /work
    if [ ! -s data/switzerland-fedlex/_all.nq ]; then
      echo 'decompressing 100 shards -> data/switzerland-fedlex/_all.nq' >&2
      zcat data/switzerland-fedlex/raw/quads/part-*.nq.gz > data/switzerland-fedlex/_all.nq
    fi
    ls -la data/switzerland-fedlex/_all.nq >&2
    ./target/release/rete build data/switzerland-fedlex/_all.nq --format nq --no-pyramid \
          -o /work/$OUT --card \
          --title 'Fedlex — Swiss federal law' \
          --license 'Fedlex terms — Swiss federal legislation is free to use and reproduce' \
          --source 'https://fedlex.data.admin.ch/sparqlendpoint' \
          --description 'The metadata layer of Swiss federal law as one range-queryable .rete: 66,392,663 quads across ~498,000 NAMED GRAPHS — one per act, consolidated version and language — harvested from Fedlex, the official publication platform of the Swiss Confederation. Modelled with JOLux (the Luxembourg-origin legislation ontology Fedlex reuses) plus ELI (European Legislation Identifier) and SKOS, whose TBox travels inside the file. Acts carry their ELI identifiers, dates, titles, languages, subject classifications (JURIVOC, legal-taxonomy), citations and amendment relations. This is the queryable INDEX of the law; the article text lives separately as Akoma Ntoso (OASIS LegalDocML) XML. Harvested by data/switzerland-fedlex/scripts/fetch_sparql.py, which works around two hard Virtuoso limits (ORDER BY capped at 10k rows, every response silently truncated at 100k) with index-backed graph enumeration and COUNT-guarded batches. Named graphs are the point — query with GRAPH ?g { … } to keep an act, its version and its language apart. Legal-domain sibling of the Spanish BOE graph; both are ELI-based.'
  "
RC=$?
echo "=== exit=$RC in $(( $(date +%s) - S ))s ===" >&2
ls -la "$OUT" 2>/dev/null | awk '{printf "  %.1f MB  %s\n", $5/1e6, $9}' >&2
