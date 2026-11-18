#!/usr/bin/env bash
# Check that every index holds all the statements: start each server on its
# index and ask COUNT(*). `oxigraph load` can exit 0 after refusing input, so
# a clean exit is not proof. Writes results/counts.tsv.
set -u
cd "$(dirname "$0")" || exit 1
export MSYS_NO_PATHCONV=1
B="$(cygpath -w "$PWD" 2>/dev/null || pwd)"
QL=adfreiburg/qlever:commit-18c097c2fb
PY=python:3.12-slim
MOUNTS=(-v "$B:/bench" -v sb-idx:/idx)
LIM=(--memory 4g --cpus 2)
GRAPHS="${GRAPHS:-linked-jazz bph davidrumsey hugging-face}"
Q='SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }'

count() { # container url
  docker run --rm --network sb-net "$PY" python3 -c '
import json, sys, time, urllib.parse, urllib.request
url, q = sys.argv[1], sys.argv[2]
for _ in range(100):
    try:
        req = urllib.request.Request(url, data=urllib.parse.urlencode({"query": q}).encode(),
                                     headers={"Accept": "application/sparql-results+json"})
        b = json.load(urllib.request.urlopen(req, timeout=900))["results"]["bindings"][0]
        print(b["n"]["value"]); break
    except OSError:
        time.sleep(0.5)
' "$2" "$Q"
}

printf 'graph\tlines\tqlever\toxigraph\n' > results/counts.tsv
for k in $GRAPHS; do
  f=$(python3 -c 'import json,sys;print(next(g["file"] for g in json.load(open("questions.json")) if g["key"]==sys.argv[1]))' "$k" 2>/dev/null || python -c 'import json,sys;print(next(g["file"] for g in json.load(open("questions.json")) if g["key"]==sys.argv[1]))' "$k")
  lines=$(docker run --rm -v sb-nt:/nt:ro "$PY" sh -c "wc -l < /nt/$f.nt")
  docker run -d --name qc "${LIM[@]}" --network sb-net "${MOUNTS[@]}" --entrypoint /qlever/qlever-server "$QL" \
    -i "/idx/qlever/$k/$k" -p 7001 -m 3G -n --no-metrics-log --no-resource-usage-log >/dev/null
  qn=$(count qc "http://qc:7001/"); docker rm -f qc >/dev/null
  docker run -d --name oc "${LIM[@]}" --network sb-net "${MOUNTS[@]}" "$PY" \
    /bench/bin/oxigraph serve-read-only --location "/idx/oxigraph/$k" --bind 0.0.0.0:7878 >/dev/null
  on=$(count oc "http://oc:7878/query"); docker rm -f oc >/dev/null
  printf '%s\t%s\t%s\t%s\n' "$k" "$lines" "$qn" "$on" | tee -a results/counts.tsv
done
