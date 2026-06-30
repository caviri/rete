#!/usr/bin/env bash
# Sanity-check a freshly built .rete: header, stats, content-hash, card, schema, and
# a spot-check SPARQL. Uses the sibling `rete` wrapper (PATH or rete-dev Docker).
#
# Usage: verify_rete.sh /work/web/foo.rete   (path as the `rete` wrapper expects)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
RETE="$HERE/rete"
FILE="${1:?usage: verify_rete.sh <file.rete>}"

echo "== rete info =="   ; "$RETE" info   "$FILE" || true
echo "== rete stats =="  ; "$RETE" stats  "$FILE" || true
echo "== rete verify ==" ; "$RETE" verify "$FILE" || { echo "!! content-hash FAILED — file is corrupt/truncated"; exit 1; }
echo "== rete card =="   ; "$RETE" card   "$FILE" 2>/dev/null || echo "(no embedded card — built without --card)"
echo "== rete schema (class-to-class) =="
"$RETE" schema "$FILE" 2>/dev/null | head -30 || true
echo "== spot-check: classes by instance count =="
"$RETE" sparql "$FILE" \
  "SELECT ?c (COUNT(*) AS ?n) WHERE { ?s a ?c } GROUP BY ?c ORDER BY DESC(?n) LIMIT 15" || true

echo
echo "OK — for ENGINE correctness (only if you changed the Rust):"
echo "   cargo test -p bench --test differential     # differential oracle vs Oxigraph"
echo "   cargo test -p rete-core                     # roundtrip / lazy==eager / fuzz"
