#!/usr/bin/env bash
# Benchmark: generate a graph, build a .rete, and time queries.
# Run inside the dev container (Docker). Usage: scripts/bench.sh [PEOPLE]
set -euo pipefail

PEOPLE="${1:-20000}"
NT=/tmp/bench.nt
RETE=/tmp/bench.rete

echo "### building release binary…"
cargo build --release -q -p rete-cli
# Use the compiled binary directly so timings exclude cargo's per-invocation
# build-check overhead.
RETE_CLI="./target/release/rete"

echo "### generating graph (~${PEOPLE} people)…"
python3 scripts/gen_graph.py "$PEOPLE" 5 100 > "$NT"
TRIPLES=$(wc -l < "$NT")
RAW=$(stat -c %s "$NT")
gzip -9 -c "$NT" > "$NT.gz"; GZ=$(stat -c %s "$NT.gz")

ms() { date +%s%3N; }
timed() { local s; s=$(ms); "$@" >/dev/null 2>/tmp/err || { cat /tmp/err; exit 1; }; echo $(( $(ms) - s )); }

echo "### build .rete"
BUILD_MS=$(timed $RETE_CLI build "$NT" -o "$RETE")
SIZE=$(stat -c %s "$RETE")

echo
echo "==================== RESULTS ===================="
printf "triples            : %s\n" "$TRIPLES"
ratio() { awk "BEGIN{printf \"%.1f\", $1/$2}"; }
printf "raw N-Triples      : %d bytes\n" "$RAW"
printf "gzip -9 N-Triples  : %d bytes (%sx)\n" "$GZ" "$(ratio "$RAW" "$GZ")"
printf ".rete (zstd+pyramid): %d bytes (%sx vs raw, %sx vs gzip)\n" "$SIZE" \
       "$(ratio "$RAW" "$SIZE")" "$(ratio "$GZ" "$SIZE")"
printf "build time         : %d ms\n" "$BUILD_MS"
$RETE_CLI info "$RETE" 2>/dev/null | grep -E "pyramid_levels|quad_count|term_count" | sed 's/^/  /'

echo
echo "---- query latency (end-to-end CLI: open + decompress + eval) ----"
printf "triple pattern (p? knows p100)   : %d ms\n" \
  "$(timed $RETE_CLI query "$RETE" --predicate '<http://ex/knows>' --object '<http://ex/p100>')"
printf "2-hop BGP join (knows . knows)   : %d ms\n" \
  "$(timed $RETE_CLI sparql "$RETE" 'PREFIX ex: <http://ex/> SELECT ?z WHERE { ex:p0 ex:knows ?y . ?y ex:knows ?z } LIMIT 50')"
printf "property path (p0 knows+ ?y)     : %d ms\n" \
  "$(timed $RETE_CLI sparql "$RETE" 'PREFIX ex: <http://ex/> SELECT ?y WHERE { ex:p0 ex:knows+ ?y } LIMIT 50')"
printf "GROUP BY COUNT (degree per node) : %d ms\n" \
  "$(timed $RETE_CLI sparql "$RETE" 'PREFIX ex: <http://ex/> SELECT ?p (COUNT(?f) AS ?n) WHERE { ?p ex:knows ?f } GROUP BY ?p')"
printf "predicate totals (summary only)  : %d ms\n" \
  "$(timed $RETE_CLI predicates "$RETE")"

echo
echo "---- HTTP range query (only the bytes a query needs) ----"
python3 scripts/range_server.py 8077 /tmp >/dev/null 2>&1 &
SRV=$!; sleep 1
$RETE_CLI query-url "http://127.0.0.1:8077/bench.rete" --object '<http://ex/p100>' 2>&1 | grep -E "result|fetched" || true
kill $SRV 2>/dev/null || true
echo "================================================="
