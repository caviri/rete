#!/usr/bin/env bash
# Measure one `rete export` against its --memory-budget-mb, without writing the dump.
#
# WHY
#   `--memory-budget-mb` is a promise about peak RSS, and the only honest test of
#   it is a full export of a large file: a 14.75 GiB openaire shard runs for
#   hours, so it cannot be a CI test (CI checks the accounting on a synthetic
#   file instead — `memory_budget_charges_the_fixed_directories`). This is the
#   reproducible dev measurement behind the table in docs/cli.md.
#
# WHAT IT DOES
#   Runs `rete export FILE --format nq --memory-budget-mb N` in a memory-capped
#   container, streams the N-Quads through sha256sum (nothing reaches the disk),
#   and records peak RSS (/usr/bin/time -v), the cgroup peak, wall time, the
#   output hash, and a RETE_MEM_TRACE line every TRACE seconds (RSS, cache
#   residency, fixed directories). One line of results goes to stdout:
#
#     budget_mb=4096 peak_rss_mb=4500 ratio=1.10 wall_s=9000 sha256=… rc=0
#
#   The same file exported at several budgets must give the same sha256.
#
# USAGE
#   scripts/export_memory_profile.sh FILE BUDGET_MB [OUT_DIR]
#
#   FILE      a .rete path; its directory is mounted read-only into the container.
#             Random reads through a Windows bind mount are very slow (rete sits
#             in D state); stage a large file into a docker volume and pass
#             RETE_DATA_VOLUME=<volume> to mount that instead (FILE is then the
#             file name inside it).
#   BUDGET_MB the --memory-budget-mb to test.
#   OUT_DIR   where logs go (default dev/export-memory/logs, gitignored).
#
# ENVIRONMENT
#   RETE_BIN          binary inside the repo (default target/release/rete; build it
#                     first with `docker compose run --rm dev cargo build --release -p rete-cli`)
#   RETE_IMAGE        image (default rete-dev:latest)
#   RETE_MEM_CAP      container memory cap (default 24g)
#   RETE_DATA_VOLUME  docker volume holding FILE (see above)
#   TRACE             RETE_MEM_TRACE interval in seconds (default 30)
set -euo pipefail

if [ $# -lt 2 ]; then
  sed -n '2,40p' "$0" >&2
  exit 2
fi
file="$1"
budget="$2"
repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${3:-$repo/dev/export-memory/logs}"
bin="${RETE_BIN:-target/release/rete}"
image="${RETE_IMAGE:-rete-dev:latest}"
cap="${RETE_MEM_CAP:-24g}"
trace="${TRACE:-30}"
mkdir -p "$out"
name="$(basename "$file" .rete)-${budget}"

if [ -n "${RETE_DATA_VOLUME:-}" ]; then
  data_mount="$RETE_DATA_VOLUME:/data:ro"
  inner="/data/$(basename "$file")"
else
  data_mount="$(cd "$(dirname "$file")" && pwd):/data:ro"
  inner="/data/$(basename "$file")"
fi
rel_out="${out#"$repo"/}"
case "$rel_out" in /*) echo "OUT_DIR must be inside the repository" >&2; exit 2 ;; esac

# MSYS (Git Bash) rewrites /paths in arguments; the container needs them verbatim.
MSYS_NO_PATHCONV=1 docker run --rm --memory "$cap" --memory-swap "$cap" \
  -v "$repo:/work" -v "$data_mount" -w /work \
  -e RETE_MEM_TRACE="$trace" -e RETE_OPEN_DEBUG=1 \
  "$image" bash -c '
    set -o pipefail
    bin="$1"; inner="$2"; budget="$3"; o="$4"; name="$5"
    s0=$(date +%s.%N)
    /usr/bin/time -v -o "$o/$name.time" "./$bin" export "$inner" --format nq \
      --memory-budget-mb "$budget" 2>"$o/$name.err" | sha256sum | cut -c1-64 >"$o/$name.sha"
    rc=$?
    s1=$(date +%s.%N)
    peak_kb=$(awk -F": " "/Maximum resident/ {print \$2}" "$o/$name.time")
    cg=$(cat /sys/fs/cgroup/memory.peak 2>/dev/null || echo 0)
    awk -v b="$budget" -v p="$peak_kb" -v a="$s0" -v z="$s1" -v s="$(cat "$o/$name.sha")" -v rc="$rc" -v cg="$cg" \
      "BEGIN { printf \"budget_mb=%d peak_rss_mb=%d ratio=%.2f wall_s=%.0f cgroup_peak_mb=%d sha256=%s rc=%d\n\", b, p/1024, p/1024/b, z-a, cg/1048576, s, rc }"
  ' _ "$bin" "$inner" "$budget" "$rel_out" "$name" | tee "$out/$name.result"
