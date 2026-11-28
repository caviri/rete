#!/usr/bin/env bash
# Benchmark rete-server, with `rete serve` (all Rust) as a reference, on local
# synthetic graphs. Everything runs in containers; nothing is fetched or
# uploaded. See README.md for what each scenario measures.
#
#   docker build -t rete-server-test clients/server
#   bash clients/server/bench/run.sh            # full: 10 s runs, 3 repetitions
#   QUICK=1 bash clients/server/bench/run.sh    # 5 s runs, once
set -euo pipefail
export MSYS_NO_PATHCONV=1  # Git Bash: leave container paths alone

here=$(cd "$(dirname "$0")" && pwd)
IMAGE=${IMAGE:-rete-server-test}
CLI_IMAGE=${CLI_IMAGE:-ghcr.io/caviri/rete-cli:0.7.2}
K6_IMAGE=${K6_IMAGE:-grafana/k6:1.0.0}
if [ "${QUICK:-0}" = 1 ]; then
  DURATION=${DURATION:-5s} REPS=${REPS:-1}
else
  DURATION=${DURATION:-10s} REPS=${REPS:-3}
fi
WARMUP=${WARMUP:-3s}
CONCURRENCY=${CONCURRENCY:-1 4 16 64}
RUST_CONCURRENCY=${RUST_CONCURRENCY:-1 4}
SCENARIOS=${SCENARIOS:-health small large count group spread api mcp large_gzip}
RUST_SCENARIOS=${RUST_SCENARIOS:-small large count group}
SERVER_CPUS=${SERVER_CPUS:-0-3}  # both servers get the same four CPUs
LOAD_CPUS=${LOAD_CPUS:-8-15}     # and the load generator others
TRIPLES=${TRIPLES:-200000}
OUT=${OUT:-$here/results/$(date +%Y%m%d-%H%M%S)}
QUERIES=$(tr -d '\n' < "$here/queries.json")
TOKEN=bench
NET=rete-bench
VOL=rete-bench-data

mkdir -p "$OUT"
results="$OUT/results.jsonl"
: > "$results"

cleanup() {
  docker rm -f rete-bench-py rete-bench-rust rete-bench-probe-py rete-bench-probe-rust >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup
docker network create "$NET" >/dev/null
docker volume create "$VOL" >/dev/null

# python SCRIPT [docker run options…]: run a bench script inside the server image.
python_in() {
  local script=$1
  shift
  docker run --rm -i --user 0 --cpuset-cpus "$SERVER_CPUS" --memory 4g -v "$VOL:/data" "$@" \
    --entrypoint python "$IMAGE" - < "$here/$script"
}

# CPU seconds, RSS and peak RSS (MiB) of a server's process, read from a probe
# container that shares its process and network namespaces.
PROBE='import os
f = open("/proc/1/stat").read().rsplit(")", 1)[1].split()
s = dict(l.split(":", 1) for l in open("/proc/1/status"))
kib = lambda k: int(s[k].split()[0]) // 1024
print((int(f[11]) + int(f[12])) / os.sysconf("SC_CLK_TCK"), kib("VmRSS"), kib("VmHWM"))'
sample() { docker exec "$1" python -c "$PROBE"; }

start_probe() { # probe server port path
  docker run -d --name "$1" --pid "container:$2" --network "container:$2" \
    --entrypoint sleep "$IMAGE" infinity >/dev/null
  for _ in $(seq 120); do
    if docker exec "$1" python -c "import urllib.request as u; u.urlopen('http://127.0.0.1:$3$4', timeout=2)" 2>/dev/null; then
      return
    fi
    sleep 0.5
  done
  echo "$2 did not come up" >&2
  docker logs "$2" >&2
  exit 1
}

k6() { # server target scenario vus duration
  docker run --rm -i --network "$NET" --cpuset-cpus "$LOAD_CPUS" \
    -e TARGET="$2" -e SERVER="$1" -e SCENARIO="$3" -e VUS="$4" -e DURATION="$5" \
    -e TOKEN="$TOKEN" -e QUERIES="$QUERIES" \
    "$K6_IMAGE" run --quiet --no-color - < "$here/load.js" | sed -n 's/^@@RESULT //p'
}

measure() { # server target probe scenario vus rep
  local t0 t1 rss hwm r
  read -r t0 _ _ < <(sample "$3")
  r=$(k6 "$1" "$2" "$4" "$5" "$DURATION")
  read -r t1 rss hwm < <(sample "$3")
  if [ -z "$r" ]; then
    echo "k6 gave no result for $1 $4 at $5 clients" >&2
    exit 1
  fi
  printf '{"kind":"load","server":"%s","scenario":"%s","vus":%s,"rep":%s,"cpu_s":%s,"rss_mb":%s,"hwm_mb":%s,"k6":%s}\n' \
    "$1" "$4" "$5" "$6" "$(awk "BEGIN{print $t1 - $t0}")" "$rss" "$hwm" "$r" | tee -a "$results"
}

sweep() { # server target probe scenarios concurrency
  local s c rep
  for s in $4; do
    k6 "$1" "$2" "$s" 4 "$WARMUP" >/dev/null
    for c in $5; do
      for rep in $(seq "$REPS"); do
        measure "$1" "$2" "$3" "$s" "$c" "$rep"
      done
    done
  done
}

server_version=$(docker run --rm --entrypoint rete-server "$IMAGE" --version)
rust_version=$(docker run --rm "$CLI_IMAGE" --version)
printf '{"kind":"meta","date":"%s","server":"%s","rust":"%s","triples":%s,"duration":"%s","reps":%s,"server_cpus":"%s","load_cpus":"%s"}\n' \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$server_version" "$rust_version" "$TRIPLES" "$DURATION" "$REPS" \
  "$SERVER_CPUS" "$LOAD_CPUS" >> "$results"

echo "== graphs"
python_in make_graphs.py -e TRIPLES="$TRIPLES"

echo "== in process"
python_in engine.py -e QUERIES="$QUERIES" | tee -a "$results"

echo "== rete-server ($server_version)"
docker run -d --name rete-bench-py --network "$NET" --cpuset-cpus "$SERVER_CPUS" --memory 4g \
  -v "$VOL:/data:ro" -e RETE_CATALOG=none -e RETE_TOKEN="$TOKEN" \
  -e RETE_WARM=local/g0.rete,local/g1.rete,local/g2.rete,local/g3.rete "$IMAGE" >/dev/null
start_probe rete-bench-probe-py rete-bench-py 7860 /health
sweep py http://rete-bench-py:7860 rete-bench-probe-py "$SCENARIOS" "$CONCURRENCY"
docker rm -f rete-bench-probe-py rete-bench-py >/dev/null

if [ -n "$RUST_SCENARIOS" ]; then
  echo "== rete serve ($rust_version)"
  docker run -d --name rete-bench-rust --network "$NET" --cpuset-cpus "$SERVER_CPUS" --memory 4g \
    -v "$VOL:/data:ro" "$CLI_IMAGE" serve /data/g0.rete --bind 0.0.0.0:8080 --journal /tmp/g0.changes >/dev/null
  start_probe rete-bench-probe-rust rete-bench-rust 8080 /
  sweep rust http://rete-bench-rust:8080 rete-bench-probe-rust "$RUST_SCENARIOS" "$RUST_CONCURRENCY"
fi

docker run --rm -i --entrypoint python "$IMAGE" -c "$(cat "$here/report.py")" < "$results" | tee "$OUT/report.md"
echo "results in $OUT"
