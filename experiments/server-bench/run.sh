#!/usr/bin/env bash
# QLever and Oxigraph on the four landing graphs: build, serve, ask the
# landing's questions. Docker only; every container gets 2 CPUs and 4 GB, and
# only one engine runs at a time. QLever gets qlever-settings.json: one
# davidrumsey literal is a 38-digit xsd:integer, which QLever otherwise refuses.
# Oxigraph loads with --lenient: one hugging-face IRI is <%20https://...>, a
# relative IRI it otherwise refuses (rete and QLever take it as it is).
#
#   GRAPHS="linked-jazz:linked-jazz-2026-06-13" ENGINES="qlever" ./run.sh
#
# Setup, once (data/, nt/ and bin/ are not tracked):
#   docker volume create sb-nt; docker volume create sb-idx; docker network create sb-net
#   for u in linked-jazz/linked-jazz-2026-06-13 bph/bph-2026-07-01 davidrumsey/davidrumsey hugging-face/hugging-face; do
#     f=$(basename "$u"); curl -o "data/$f.rete" "https://data.graphplaza.com/$u.rete"
#     docker run --rm -v "$PWD/data:/data:ro" ghcr.io/caviri/rete-cli:0.7.4 export --format nq "/data/$f.rete" > "nt/$f.nt"
#   done
#   docker run --rm -v "$PWD/nt:/src:ro" -v sb-nt:/nt alpine:3.20 sh -c 'cp /src/*.nt /nt/'
#   docker run --rm -v sb-idx:/idx alpine:3.20 sh -c 'mkdir -p /idx/qlever /idx/oxigraph && chmod -R 777 /idx'
#   docker create --name oxcp ghcr.io/oxigraph/oxigraph:0.5.13 && docker cp oxcp:/usr/local/bin/oxigraph bin/ && docker rm oxcp
#
# Every graph is default-graph only, so the exported N-Quads lines are
# N-Triples. Results: results/*.json, folded by summarize.py.
set -u
cd "$(dirname "$0")" || exit 1
export MSYS_NO_PATHCONV=1   # Git Bash on Windows: keep container paths as written
B="$(cygpath -w "$PWD" 2>/dev/null || pwd)"
QL=adfreiburg/qlever:commit-18c097c2fb   # = latest on 2026-11-14
OX_HOST=python:3.12-slim                  # runs the binary copied from ghcr.io/oxigraph/oxigraph:0.5.13 (bin/oxigraph)
PY=python:3.12-slim
GRAPHS="${GRAPHS:-linked-jazz:linked-jazz-2026-06-13 bph:bph-2026-07-01 davidrumsey:davidrumsey hugging-face:hugging-face}"
ENGINES="${ENGINES:-qlever oxigraph}"
MOUNTS=(-v "$B:/bench" -v sb-nt:/nt:ro -v sb-idx:/idx)
CPUS="${CPUS:-2}"   # every machine priced on the landing has 2 vCPUs; Oxigraph's loader runs out of 4 GB on 32
LIM=(--memory 4g --cpus "$CPUS")
R=/bench/results

log() { echo "[$(date +%H:%M:%S)] $*"; }

# shellcheck disable=SC2016  # $(cat ...) belongs to the shell inside the container
vm() { docker exec "$1" sh -c 'grep -E "^Vm(RSS|HWM)" /proc/$(cat /tmp/child.pid)/status | tr -s " \t" " " | tr "\n" " "'; }

ready() { # container url -> seconds until the endpoint answers
  local t0
  t0=$(date +%s.%N)
  until docker exec "$1" python3 -c "import urllib.request,sys;urllib.request.urlopen(sys.argv[1],timeout=2)" "$2" >/dev/null 2>&1; do
    docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null | grep -q true || { echo "dead"; return 1; }
    sleep 0.2
  done
  awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}'
}

build() { # engine key file
  local e=$1 k=$2 f=$3 name="bld-$1-$2"
  docker rm -f "$name" >/dev/null 2>&1
  if [ "$e" = qlever ]; then
    docker run --rm "${MOUNTS[@]}" --entrypoint sh "$QL" -c "rm -rf /idx/qlever/$k && mkdir -p /idx/qlever/$k"
    docker run --name "$name" "${LIM[@]}" "${MOUNTS[@]}" -w "/idx/qlever/$k" --entrypoint python3 "$QL" \
      /bench/measure.py "$R/$e-$k-build.json" -- /qlever/qlever-index -i "/idx/qlever/$k/$k" -f "/nt/$f.nt" \
      -F nt -p true -m 1G -s /bench/qlever-settings.json > "results/$e-$k-build.log" 2>&1
  else
    docker run --rm "${MOUNTS[@]}" "$PY" sh -c "rm -rf /idx/oxigraph/$k"
    docker run --name "$name" "${LIM[@]}" "${MOUNTS[@]}" "$OX_HOST" \
      python3 /bench/measure.py "$R/$e-$k-build.json" -- /bench/bin/oxigraph load --lenient \
      --location "/idx/oxigraph/$k" --file "/nt/$f.nt" > "results/$e-$k-build.log" 2>&1
  fi
  log "$e $k build exit=$(docker inspect -f '{{.State.ExitCode}} oom={{.State.OOMKilled}}' "$name")"
  docker inspect -f '{{.State.OOMKilled}}' "$name" > "results/$e-$k-build.oom"
  docker rm "$name" >/dev/null
  docker run --rm -v sb-idx:/idx "$PY" sh -c "du -sb /idx/$e/$k | cut -f1; find /idx/$e/$k -maxdepth 1 -type f -printf '%s %f\n' | sort -nr" \
    > "results/$e-$k-index.txt"
  log "$e $k index $(head -1 "results/$e-$k-index.txt") bytes"
}

serve() { # engine key
  local e=$1 k=$2 name url ep
  if [ "$e" = qlever ]; then
    name=qs; url="http://localhost:7001/?cmd=stats"; ep="http://qs:7001/"
    docker run -d --name "$name" "${LIM[@]}" --network sb-net "${MOUNTS[@]}" --entrypoint python3 "$QL" \
      /bench/measure.py "$R/$e-$k-serve.json" -- /qlever/qlever-server -i "/idx/qlever/$k/$k" -p 7001 -m 3G -n \
      --no-metrics-log --no-resource-usage-log >/dev/null
  else
    name=os; url="http://localhost:7878/"; ep="http://os:7878/query"
    docker run -d --name "$name" "${LIM[@]}" --network sb-net "${MOUNTS[@]}" "$OX_HOST" \
      python3 /bench/measure.py "$R/$e-$k-serve.json" -- /bench/bin/oxigraph serve-read-only \
      --location "/idx/oxigraph/$k" --bind 0.0.0.0:7878 >/dev/null
  fi
  local up idle after
  up=$(ready "$name" "$url") || { log "$e $k server died"; docker logs "$name" | tail -20; docker rm -f "$name" >/dev/null; return; }
  idle=$(vm "$name")
  log "$e $k ready in ${up}s, $idle"
  docker run --rm --network sb-net "${MOUNTS[@]}" "$PY" python3 /bench/client.py "$e" "$ep" "$k" "$R/$e-$k-queries.json" | sed "s/^/    /"
  after=$(vm "$name")
  docker stop -t 40 "$name" >/dev/null
  docker logs "$name" > "results/$e-$k-serve.log" 2>&1
  docker rm "$name" >/dev/null
  printf '{"ready_s": %s, "idle": "%s", "after": "%s"}\n' "$up" "$idle" "$after" > "results/$e-$k-serve-snap.json"
  log "$e $k after questions: $after"
}

for gk in $GRAPHS; do
  k=${gk%%:*}; f=${gk#*:}
  for e in $ENGINES; do
    build "$e" "$k" "$f"
    serve "$e" "$k"
  done
done
