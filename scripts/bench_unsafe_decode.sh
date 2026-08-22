#!/usr/bin/env bash
# Safe/unchecked comparison for the three Chemotion catalog queries.
#
# Build first:
#   cargo build --release -p rete-cli --features unsafe-decode-bench
# Optional environment: RETE_EXE, RETE_SOURCE, RETE_SAMPLES, RETE_ONLY.
# RETE_SOURCE may be the catalog R2 URL or a trusted local .rete path.
set -euo pipefail

exe=${RETE_EXE:-/target/release/rete}
source=${RETE_SOURCE:-https://data.graphplaza.com/chemotion/chemotion.rete}
samples=${RETE_SAMPLES:-7}
tmp=$(mktemp -d)

labels=(select aggregate path)
queries=(
'PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX obo: <http://purl.obolibrary.org/obo/>
PREFIX chebi: <http://purl.obolibrary.org/obo/chebi/>
SELECT ?name ?formula ?smiles WHERE {
  ?m a obo:CHEBI_23367 ; rdfs:label ?name ; chebi:formula ?formula ; chebi:smiles ?smiles
} LIMIT 200'
'PREFIX chebi: <http://purl.obolibrary.org/obo/chebi/>
SELECT ?formula (COUNT(?m) AS ?molecules) WHERE {
  ?m chebi:formula ?formula
} GROUP BY ?formula ORDER BY DESC(?molecules) LIMIT 20'
'PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?name WHERE {
  ?sub rdfs:subClassOf+ <http://purl.obolibrary.org/obo/CHMO_0000228> ; rdfs:label ?name
} ORDER BY ?name LIMIT 200'
)

run_one() {
    local label=$1 mode=$2 run=$3 query=$4
    local out="$tmp/$label-$mode-$run.json"
    local err="$tmp/$label-$mode-$run.err"
    local start stop ms hash
    start=$(date +%s%N)
    if [[ $mode == unsafe ]]; then
        "$exe" sparql-url "$source" "$query" --json --unsafe-decode >"$out" 2>"$err"
    else
        "$exe" sparql-url "$source" "$query" --json >"$out" 2>"$err"
    fi
    stop=$(date +%s%N)
    ms=$(( (stop - start) / 1000000 ))
    hash=$(sha256sum "$out" | cut -d' ' -f1)
    printf 'SAMPLE %s %s run=%s ms=%s hash=%s stats=%s\n' \
        "$label" "$mode" "$run" "$ms" "$hash" "$(tr '\n' ' ' <"$err")"
}

for i in "${!labels[@]}"; do
    label=${labels[$i]}
    query=${queries[$i]}
    if [[ -n ${RETE_ONLY:-} && $label != "$RETE_ONLY" ]]; then
        continue
    fi
    "$exe" sparql-url "$source" "$query" --json >"$tmp/$label-safe-identity.json" 2>/dev/null
    "$exe" sparql-url "$source" "$query" --json --unsafe-decode >"$tmp/$label-unsafe-identity.json" 2>/dev/null
    cmp "$tmp/$label-safe-identity.json" "$tmp/$label-unsafe-identity.json"
    printf 'IDENTITY %s %s\n' "$label" \
        "$(sha256sum "$tmp/$label-safe-identity.json" | cut -d' ' -f1)"
    "$exe" sparql-url "$source" "$query" --json >/dev/null 2>/dev/null
    "$exe" sparql-url "$source" "$query" --json --unsafe-decode >/dev/null 2>/dev/null
    for run in $(seq 1 "$samples"); do
        if (( run % 2 == 1 )); then
            run_one "$label" safe "$run" "$query"
            run_one "$label" unsafe "$run" "$query"
        else
            run_one "$label" unsafe "$run" "$query"
            run_one "$label" safe "$run" "$query"
        fi
    done
done
