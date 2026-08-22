#!/usr/bin/env bash
# Alternate two rete builds over one pinned RDF input and verify byte identity.
# Required: RETE_BASELINE_EXE, RETE_OPTIMIZED_EXE, RETE_BUILD_SOURCE.
# Optional: RETE_SAMPLES (default 5), RETE_BUILD_DIR (default /tmp).
set -euo pipefail

baseline=${RETE_BASELINE_EXE:?set RETE_BASELINE_EXE}
optimized=${RETE_OPTIMIZED_EXE:?set RETE_OPTIMIZED_EXE}
source=${RETE_BUILD_SOURCE:?set RETE_BUILD_SOURCE}
samples=${RETE_SAMPLES:-5}
out_dir=${RETE_BUILD_DIR:-/tmp/rete-encoder-bench}
mkdir -p "$out_dir"

run_one() {
    local name=$1 exe=$2 run=$3
    local out="$out_dir/$name-$run.rete"
    local log="$out_dir/$name-$run.log"
    local start stop ms hash size
    start=$(date +%s%N)
    "$exe" build "$source" -o "$out" --pyramid-algo types --card >"$log" 2>&1
    stop=$(date +%s%N)
    ms=$(( (stop - start) / 1000000 ))
    hash=$(sha256sum "$out" | cut -d' ' -f1)
    size=$(wc -c <"$out")
    printf 'BUILD %s run=%s ms=%s bytes=%s hash=%s\n' \
        "$name" "$run" "$ms" "$size" "$hash"
}

run_one baseline "$baseline" warmup
run_one optimized "$optimized" warmup
for run in $(seq 1 "$samples"); do
    if (( run % 2 == 1 )); then
        run_one baseline "$baseline" "$run"
        run_one optimized "$optimized" "$run"
    else
        run_one optimized "$optimized" "$run"
        run_one baseline "$baseline" "$run"
    fi
done
