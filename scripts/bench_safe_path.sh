#!/usr/bin/env bash
set -euo pipefail

: "${RETE_SOURCE:?set RETE_SOURCE to the pinned local .rete file}"
: "${RETE_BASE_EXE:?set RETE_BASE_EXE to the immutable baseline executable}"
: "${RETE_CANDIDATE_EXE:?set RETE_CANDIDATE_EXE to the candidate executable}"
RETE_SAMPLES="${RETE_SAMPLES:-15}"

[[ -f "$RETE_SOURCE" ]] || { echo "missing source: $RETE_SOURCE" >&2; exit 2; }
[[ -x "$RETE_BASE_EXE" ]] || { echo "missing baseline: $RETE_BASE_EXE" >&2; exit 2; }
[[ -x "$RETE_CANDIDATE_EXE" ]] || { echo "missing candidate: $RETE_CANDIDATE_EXE" >&2; exit 2; }
[[ "$RETE_SAMPLES" =~ ^[1-9][0-9]*$ ]] || { echo "RETE_SAMPLES must be positive" >&2; exit 2; }

bench_tmp="$(mktemp -d)"
trap 'rm -rf -- "$bench_tmp"' EXIT
raw="$bench_tmp/raw.tsv"

names=(path full_count selective aggregate)
queries=(
  'PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> SELECT ?name WHERE { ?sub rdfs:subClassOf+ <http://purl.obolibrary.org/obo/CHMO_0000228> ; rdfs:label ?name } ORDER BY ?name LIMIT 200'
  'SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }'
  'PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX obo: <http://purl.obolibrary.org/obo/> PREFIX chebi: <http://purl.obolibrary.org/obo/chebi/> SELECT ?name ?formula ?smiles WHERE { ?m a obo:CHEBI_23367 ; rdfs:label ?name ; chebi:formula ?formula ; chebi:smiles ?smiles } ORDER BY ?name ?formula ?smiles LIMIT 200'
  'PREFIX chebi: <http://purl.obolibrary.org/obo/chebi/> SELECT ?formula (COUNT(?m) AS ?molecules) WHERE { ?m chebi:formula ?formula } GROUP BY ?formula ORDER BY DESC(?molecules) ?formula LIMIT 20'
)

measure() {
  local exe="$1" name="$2" mode="$3" query="$4" sample="$5"
  local output="$bench_tmp/output.json" start end elapsed digest
  start="$(date +%s%N)"
  "$exe" sparql "$RETE_SOURCE" "$query" --json >"$output"
  end="$(date +%s%N)"
  elapsed="$(( (end - start) / 1000000 ))"
  digest="$(sha256sum "$output" | cut -d' ' -f1)"
  printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$mode" "$sample" "$elapsed" "$digest" >>"$raw"
}

for i in "${!names[@]}"; do
  name="${names[$i]}"
  query="${queries[$i]}"
  "$RETE_BASE_EXE" sparql "$RETE_SOURCE" "$query" --json >/dev/null
  "$RETE_CANDIDATE_EXE" sparql "$RETE_SOURCE" "$query" --json >/dev/null
  for ((sample = 1; sample <= RETE_SAMPLES; sample++)); do
    if (( sample % 2 == 1 )); then
      measure "$RETE_BASE_EXE" "$name" baseline "$query" "$sample"
      measure "$RETE_CANDIDATE_EXE" "$name" candidate "$query" "$sample"
    else
      measure "$RETE_CANDIDATE_EXE" "$name" candidate "$query" "$sample"
      measure "$RETE_BASE_EXE" "$name" baseline "$query" "$sample"
    fi
  done
done

if [[ -n "${RETE_RAW_OUT:-}" ]]; then
  python3 -P - "$raw" "$RETE_RAW_OUT" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
destination = pathlib.Path(sys.argv[2])
with destination.open("xb") as output:
    output.write(source.read_bytes())
PY
fi

python3 -P - "$raw" <<'PY'
import collections
import math
import pathlib
import statistics
import sys

rows = collections.defaultdict(lambda: collections.defaultdict(list))
hashes = collections.defaultdict(set)
for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    name, mode, _sample, elapsed, digest = line.split("\t")
    rows[name][mode].append(int(elapsed))
    hashes[name].add(digest)

print("workload\tbaseline_median_ms\tcandidate_median_ms\tdelta_pct\tbaseline_p90_ms\tcandidate_p90_ms\tsha256")
for name, modes in rows.items():
    if len(hashes[name]) != 1:
        raise SystemExit(f"output hash mismatch for {name}: {sorted(hashes[name])}")
    base = sorted(modes["baseline"])
    candidate = sorted(modes["candidate"])
    if len(base) != len(candidate):
        raise SystemExit(f"unbalanced samples for {name}")
    median_base = statistics.median(base)
    median_candidate = statistics.median(candidate)
    p90_index = max(0, math.ceil(0.9 * len(base)) - 1)
    delta = (median_candidate - median_base) * 100.0 / median_base
    print(
        f"{name}\t{median_base:.1f}\t{median_candidate:.1f}\t{delta:+.1f}%\t"
        f"{base[p90_index]}\t{candidate[p90_index]}\t{next(iter(hashes[name]))}"
    )
PY
