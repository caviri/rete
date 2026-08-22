#!/usr/bin/env bash
# Which published .rete files are actually BROKEN today, and which are merely
# dated? Reads only each file's Dataset Card — the header plus one coalesced
# metadata range, two range requests, tens of KB — never the dictionary or the
# index. Surveying the whole 98-dataset catalog costs a couple of MB.
#
# The verdict that matters is ZERO-ROWS: a file whose statements all live in
# named graphs, whose default graph is therefore empty, and whose starter
# queries were written for the default graph. Those queries return zero rows,
# which reads to a newcomer as a broken file. (That is the bug the Czech
# national catalogue hit with nkod.rete.)
#
# The second verdict that matters is EMPTY-QUERY: the scope is right, and a
# named starter query still cannot match — because it conjoins vocabulary the
# card never proved co-occurs (issue #172: the most frequent class joined to the
# most frequent label predicate, a hub IRI joined to a top predicate, a fixed
# geometry path on a file that hangs its WKT elsewhere). `rete card-audit`
# decides that from the very card this script already fetched, so it costs no
# extra network and shares its judgement with the query generator.
#
# Verdicts, worst first:
#   CARDLESS       no Dataset Card at all — nothing describes the file
#   ZERO-ROWS      named-graph-only, but starter queries scan the default graph
#   EMPTY-QUERY    a shipped starter query the card's own profile refutes
#   MIXED-HIDDEN   data in both, but no starter query looks inside a named graph
#   SUSPECT-QUERY  a join the card can neither witness nor refute — run it
#   DATED          scope-correct, but no build record / profile / smoke query
#   CURRENT        nothing to do
#   UNREADABLE     the card tier itself failed (HTTP, format, parse)
#
# Usage:
#   bash scripts/recard/survey.sh                       # the whole catalog
#   bash scripts/recard/survey.sh --keys "nkod cordis"  # named keys only
#   bash scripts/recard/survey.sh --url https://…/x.rete --key x
#   bash scripts/recard/survey.sh --local /work/data    # every .rete under a dir
#   bash scripts/recard/survey.sh --cards <out>/cards   # re-decide, ZERO network
#
# Writes  <out>/survey.tsv, <out>/survey.json, <out>/todo.txt (keys to fix,
# worst first) and <out>/cards/<key>.json (each card as fetched).
#
# Env: RETE_IMAGE (default rete-dev:latest), RETE_BIN (/work/target/release/rete)
set -euo pipefail

if [ "${RECARD_INNER:-}" != "1" ]; then
  export MSYS_NO_PATHCONV=1
  repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  image="${RETE_IMAGE:-rete-dev:latest}"
  docker image inspect "$image" >/dev/null 2>&1 || {
    echo "survey: docker image '$image' not found (docker compose build dev)" >&2; exit 127; }
  mounts=(-v "$repo:/work")
  if [ -n "${RECARD_MOUNT:-}" ]; then
    IFS=';' read -r -a extra <<<"$RECARD_MOUNT"
    for m in "${extra[@]}"; do if [ -n "$m" ]; then mounts+=(-v "$m"); fi; done
  fi
  exec docker run --rm "${mounts[@]}" -w /work \
    -e RECARD_INNER=1 -e RETE_BIN -e TZ=UTC \
    "$image" bash /work/scripts/recard/survey.sh "$@"
fi

RETE="${RETE_BIN:-/work/target/release/rete}"
TOOLS=/work/scripts/recard/card_tools.py
catalog=/work/web/playground-src/catalog.js
out=/work/dev/recard/survey
jobs=8; keys=""; one_url=""; one_key=""; local_dir=""; include_shards=0; cards_dir=""

die() { echo "survey: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --catalog) catalog="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --jobs) jobs="$2"; shift 2 ;;
    --keys) keys="$2"; shift 2 ;;
    --url) one_url="$2"; shift 2 ;;
    --key) one_key="$2"; shift 2 ;;
    --local) local_dir="$2"; shift 2 ;;
    --cards) cards_dir="$2"; shift 2 ;;
    --include-shards) include_shards=1; shift ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done
[ -x "$RETE" ] || die "rete binary not found at $RETE (set RETE_BIN)"
mkdir -p "$out/cards"
targets="$out/targets.tsv"
: > "$targets"

if [ -n "$cards_dir" ]; then
  # Cards already on disk from an earlier run: re-decide them without touching
  # the network. The fetch is the expensive half and the judgement is the half
  # that changes, so this is the loop to iterate in.
  find "$cards_dir" -maxdepth 1 -name '*.json' ! -name '*.audit.json' -type f | sort \
    | while read -r f; do printf '%s\t%s\n' "$(basename "$f" .json)" "$f"; done >> "$targets"
elif [ -n "$one_url" ]; then
  printf '%s\t%s\n' "${one_key:-$(basename "$one_url" .rete)}" "$one_url" >> "$targets"
elif [ -n "$local_dir" ]; then
  find "$local_dir" -name '*.rete' -type f | sort | while read -r f; do
    printf '%s\t%s\n' "$(basename "$f" .rete)" "$f"
  done >> "$targets"
else
  [ -f "$catalog" ] || die "catalog not found: $catalog"
  # The catalog is a plain script assigning window.RETE_PLAYGROUND_CATALOG; run
  # it in a sandboxed VM rather than regexing it (same trick as
  # scripts/check_dataset_catalog.py). A sharded dataset is one logical graph in
  # N files; each file carries its own card, so each is its own target.
  # shellcheck disable=SC2016  # this is JavaScript; $/`` must not be shell-expanded
  node -e '
const fs=require("fs"), vm=require("vm");
const sandbox={window:{}};
vm.runInNewContext(fs.readFileSync(process.argv[1],"utf8"), sandbox, {filename:process.argv[1]});
const c=sandbox.window.RETE_PLAYGROUND_CATALOG;
const base=(c.remoteBase||"").replace(/\/$/,"");
const shards=process.argv[2]==="1";
for (const d of c.datasets||[]) {
  if (d.shards && d.shards.length) {
    if (!shards) { console.log(`${d.key}\t${d.shards[0]}`); continue; }
    d.shards.forEach((u,i)=>console.log(`${d.key}#${i+1}\t${u}`));
    continue;
  }
  console.log(`${d.key}\t${d.url || `${base}/${d.key}/${d.key}.rete`}`);
}' "$catalog" "$include_shards" >> "$targets"
fi

if [ -n "$keys" ]; then
  wanted="$out/.wanted"; : > "$wanted"
  for k in $keys; do printf '%s\n' "$k" >> "$wanted"; done
  awk -F'\t' 'NR==FNR{w[$1];next} ($1 in w)' "$wanted" "$targets" > "$targets.f"
  mv "$targets.f" "$targets"; rm -f "$wanted"
fi

total=$(wc -l < "$targets")
[ "$total" -gt 0 ] || die "no targets"
echo "== surveying $total file(s), $jobs at a time — card tier only (2 range requests each)"

probe() {
  local key="$1" url="$2"
  local card="$out/cards/$key.json" err="$out/cards/$key.err"
  local audit="$out/cards/$key.audit.json"
  if [ -n "$cards_dir" ]; then
    # Already-fetched card: decide only.
    "$RETE" card-audit "$url" --json > "$audit" 2>/dev/null || : > "$audit"
    python3 -P "$TOOLS" classify "$url" --key "$key" --url "$url" --audit "$audit"
    return 0
  fi
  # `card-url` also serves local paths (same ranged reader), so --local works.
  if "$RETE" card-url "$url" --json > "$card" 2>"$err"; then
    grep -oE '^fetched [0-9]+ of [0-9]+ bytes in [0-9]+ range request' "$err" \
      | head -1 > "$out/cards/$key.bytes" || true
    # Do the starter queries this card ALREADY ships still answer? Decided
    # from the card just fetched — no further network, and no second copy of
    # the emptiness rule (`rete card-audit` is the generator's own judgement).
    "$RETE" card-audit "$card" --json > "$audit" 2>/dev/null || : > "$audit"
    python3 -P "$TOOLS" classify "$card" --key "$key" --url "$url" --audit "$audit"
  else
    printf '{"key":%s,"url":%s,"status":"UNREADABLE","reason":%s,"triples":null,"quads":null,"named_graphs":null,"queries":0,"graph_scoped":0,"has_build_record":false,"title":null,"format_version":null}\n' \
      "$(python3 -c 'import json,sys;print(json.dumps(sys.argv[1]))' "$key")" \
      "$(python3 -c 'import json,sys;print(json.dumps(sys.argv[1]))' "$url")" \
      "$(python3 -c 'import json,sys;print(json.dumps(sys.argv[1][:200]))' "$(tail -1 "$err" 2>/dev/null || echo failed)")"
  fi
}

rows="$out/rows.jsonl"; : > "$rows"
n=0
while IFS=$'\t' read -r key url; do
  [ -n "$key" ] || continue
  probe "$key" "$url" >> "$rows" &
  n=$((n + 1))
  if [ $((n % jobs)) -eq 0 ]; then wait; fi
done < "$targets"
wait

python3 -P "$TOOLS" report "$rows" --todo "$out/todo.txt" --json "$out/survey.json" \
  > "$out/survey.tsv"

echo
column -t -s $'\t' "$out/survey.tsv" 2>/dev/null || cat "$out/survey.tsv"
echo
# Two range requests per file; the byte cost is the card's own size. One
# outlier is expected: a file with a big non-card section (embedded PMTiles)
# ahead of its metadata makes the coalesced range huge.
cat "$out"/cards/*.bytes 2>/dev/null | awk -v n="$total" '
  {s += $2; if ($2 > mx) {mx = $2}}
  END {printf "== fetched %.1f KB over %d file(s) — %.1f KB each on average, %.1f KB worst\n",
         s/1024, n, (n ? s/n : 0)/1024, mx/1024}'
echo "== $out/survey.tsv  $out/survey.json  $out/todo.txt"
