#!/usr/bin/env bash
# Re-card MANY published .rete files: the catalog-scale driver around
# recard.sh. Resumable and idempotent — a finished dataset is skipped without
# re-downloading it (recard.sh consults its receipt before spending bandwidth),
# and a dataset that died half way leaves the original untouched, because
# recard.sh installs its output only after both proofs pass.
#
# Typical run:
#   bash scripts/recard/survey.sh                     # what is actually broken
#   bash scripts/recard/recard_batch.sh --list /work/dev/recard/survey/todo.txt
#
# What it costs: every re-card rewrites the file, so every remote file has to be
# downloaded in full and (if you publish it) uploaded in full. CPU is not the
# expense here — bandwidth is. Use --max-mb to stay inside a budget, or --mirror
# (with RECARD_MOUNT) to point at a local copy you already have and skip the
# download entirely. See README "What this costs".
#
# Usage:
#   bash scripts/recard/recard_batch.sh --list FILE    [options]
#   bash scripts/recard/recard_batch.sh --keys "a b c" [options]
#
# Options:
#   --list FILE          keys, one per line (survey.sh writes todo.txt)
#   --keys "a b c"       keys inline; `key#N` names one SHARD (see below)
#   --catalog PATH       catalog.js (default /work/web/playground-src/catalog.js)
#   --mirror DIR        use DIR/<name>/<name>.rete or DIR/<name>.rete if it exists
#   --sha256-dir DIR     DIR/<name>.sha256 anchors the data proof to those exact
#                        bytes (recard.sh --expect-sha256); missing file = no
#                        anchor for that one, which is reported, not silent
#   --out-dir DIR        where rebuilt files go (default /work/dev/recard/out)
#   --work DIR           scratch + receipts (default /work/dev/recard)
#   --mode auto|repyramid|stream
#   --pyramid-algo auto|louvain|types
#                        passed through; auto (default) reproduces each source's
#                        own pyramid rather than imposing repyramid's louvain
#   --text-index auto|yes|no   passed through; auto keeps a source's index
#   --stream-above-mb N  auto switches to stream above N MB (default 192)
#   --max-mb N           skip any source larger than N MB (0 = no limit)
#   --allow-empty "ids"  starter queries permitted to return 0 rows (see README)
#   --jobs N             datasets in parallel (default 1 — each is RAM-hungry)
#   --dry-run            print the plan, touch nothing
#   --stop-on-error      abort on the first failure (default: continue)
#
# A key may name ONE SHARD of a sharded dataset as `key#N` (0-based, the index
# into the catalog's `shards` array) — e.g. `deps-dev#6`. Without that a sharded
# dataset resolves to its first shard only, which is how a survey can miss the
# one shard that carries the broken query (deps-dev's starter queries are on
# shard 6, `deps-dev-cargo`). The work is named after the shard's own file, not
# the dataset key, so two shards never share a receipt, a log or an output path.
#
# All paths are CONTAINER paths (the repo is /work); the script re-executes
# itself inside the rete dev image, and runs recard.sh in that same container.
set -euo pipefail

if [ "${RECARD_INNER:-}" != "1" ]; then
  export MSYS_NO_PATHCONV=1
  repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  image="${RETE_IMAGE:-rete-dev:latest}"
  docker image inspect "$image" >/dev/null 2>&1 || {
    echo "recard_batch: docker image '$image' not found (docker compose build dev)" >&2; exit 127; }
  mounts=(-v "$repo:/work")
  if [ -n "${RECARD_MOUNT:-}" ]; then
    IFS=';' read -r -a extra <<<"$RECARD_MOUNT"
    for m in "${extra[@]}"; do if [ -n "$m" ]; then mounts+=(-v "$m"); fi; done
  fi
  exec docker run --rm "${mounts[@]}" -w /work \
    -e RECARD_INNER=1 -e RETE_BIN -e TZ=UTC \
    "$image" bash /work/scripts/recard/recard_batch.sh "$@"
fi

here=/work/scripts/recard
list=""; keys=""; catalog=/work/web/playground-src/catalog.js
mirror=""; out_dir=/work/dev/recard/out; work=/work/dev/recard
mode="auto"; stream_above_mb=192; max_mb=0; jobs=1; dry=0; stop=0; allow_empty=""
sha_dir=""; pyramid_algo="auto"; text_index="auto"

die() { echo "recard_batch: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --list) list="$2"; shift 2 ;;
    --keys) keys="$2"; shift 2 ;;
    --catalog) catalog="$2"; shift 2 ;;
    --mirror) mirror="$2"; shift 2 ;;
    --sha256-dir) sha_dir="$2"; shift 2 ;;
    --out-dir) out_dir="$2"; shift 2 ;;
    --work) work="$2"; shift 2 ;;
    --mode) mode="$2"; shift 2 ;;
    --pyramid-algo) pyramid_algo="$2"; shift 2 ;;
    --text-index) text_index="$2"; shift 2 ;;
    --stream-above-mb) stream_above_mb="$2"; shift 2 ;;
    --max-mb) max_mb="$2"; shift 2 ;;
    --allow-empty) allow_empty="$2"; shift 2 ;;
    --jobs) jobs="$2"; shift 2 ;;
    --dry-run) dry=1; shift ;;
    --stop-on-error) stop=1; shift ;;
    -h|--help) sed -n '2,52p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

want=()
if [ -n "$list" ]; then
  [ -f "$list" ] || die "list not found: $list"
  while read -r k; do if [ -n "$k" ]; then want+=("$k"); fi; done < "$list"
fi
for k in $keys; do want+=("$k"); done
[ ${#want[@]} -gt 0 ] || die "nothing to do (pass --list or --keys)"

# Resolve key -> URL from the catalog, and key -> size from the release lock
# (web/datasets.lock.json), so the plan can state the download bill up front.
plan="$work/batch/plan.tsv"
mkdir -p "$work/batch"
python3 -P - "$catalog" "$mirror" "${want[@]}" > "$plan" <<'PY'
import json, os, subprocess, sys
catalog, mirror, *keys = sys.argv[1:]
node = r"""
const fs=require("fs"), vm=require("vm");
const sb={window:{}};
vm.runInNewContext(fs.readFileSync(process.argv[1],"utf8"), sb, {filename:process.argv[1]});
process.stdout.write(JSON.stringify(sb.window.RETE_PLAYGROUND_CATALOG));
"""
cat = json.loads(subprocess.run(["node", "-e", node, catalog],
                                capture_output=True, text=True, check=True).stdout)
base = (cat.get("remoteBase") or "").rstrip("/")
by_key, shards_of = {}, {}
for d in cat.get("datasets", []):
    key = d["key"]
    shards_of[key] = d.get("shards") or []
    by_key[key] = d["shards"][0] if d.get("shards") else (
        d.get("url") or (f"{base}/{key}/{key}.rete" if base else ""))
sizes = {}
lock = "/work/web/datasets.lock.json"
if os.path.exists(lock):
    for d in json.load(open(lock)).get("datasets", []):
        sizes[d["key"]] = d.get("size", 0)
def remote_size(url):
    """One HEAD per unknown file, so the plan states the real download bill."""
    try:
        out = subprocess.run(["curl", "-fsSLI", url], capture_output=True,
                             text=True, timeout=60).stdout
    except Exception:
        return 0
    for line in out.splitlines():
        if line.lower().startswith("content-length:"):
            try:
                return int(line.split(":", 1)[1].strip())
            except ValueError:
                pass
    return 0


for spec in keys:
    # `key#N` names one shard. A sharded dataset otherwise resolves to shards[0],
    # which is exactly how a first-shard survey misses the shard that is broken.
    key, _, shard = spec.partition("#")
    src, size = by_key.get(key, ""), sizes.get(key, 0)
    name = key
    if shard != "":
        picks = shards_of.get(key) or []
        try:
            src = picks[int(shard)]
        except (ValueError, IndexError):
            print(f"{spec}\t-\t0")
            continue
        # The shard's own basename, so two shards never share a receipt, a log or
        # an output path — and so the name matches the object key on the host.
        name, size = os.path.basename(src)[: -len(".rete")], 0
    # A local mirror wins over the network whenever the file is really there.
    if mirror:
        for cand in (f"{mirror}/{name}/{name}.rete", f"{mirror}/{name}.rete"):
            if os.path.exists(cand):
                src, size = cand, os.path.getsize(cand)
                break
    if not size and src.startswith("http"):
        size = remote_size(src)
    # `-` for "no such key": tab is IFS whitespace, so an empty middle field
    # would silently collapse and shift the shell's `read` by one column.
    print(f"{name}\t{src or '-'}\t{size}")
PY

n=0; total_bytes=0; unknown=0; remote=0; remote_bytes=0
while IFS=$'\t' read -r key src size; do
  if [ "$src" = "-" ]; then echo "!! $key: not in the catalog — skipping" >&2; continue; fi
  n=$((n + 1)); total_bytes=$((total_bytes + size))
  case "$src" in http*) remote=$((remote + 1)); remote_bytes=$((remote_bytes + size)) ;; esac
  [ "$size" = 0 ] && unknown=$((unknown + 1))
done < "$plan"
gb() { awk "BEGIN{printf \"%.2f GB\", $1/1073741824}"; }
note=""
[ "$unknown" -gt 0 ] && note=" ($unknown of unknown size)"
echo "== plan: $n dataset(s), $(gb "$total_bytes") of source$note"
if [ "$remote" -gt 0 ]; then
  echo "== $remote of them come over the network: $(gb "$remote_bytes") down, and the same"
  echo "== again up if you publish the result. Bandwidth is the bill here, not CPU."
else
  echo "== all sources are local (--mirror) — no download"
fi
if [ "$dry" = 1 ]; then
  column -t -s $'\t' "$plan" 2>/dev/null || cat "$plan"
  exit 0
fi

summary="$work/batch/summary.tsv"
: > "$summary"

run_one() {
  local key="$1" src="$2" size="$3"
  local log="$work/batch/$key.log"
  if [ "$max_mb" != 0 ] && [ "$size" != 0 ] && [ "$size" -gt $((max_mb * 1024 * 1024)) ]; then
    printf '%s\tSKIPPED\ttoo large (%s bytes > --max-mb %s)\n' "$key" "$size" "$max_mb" >> "$summary"
    echo "-- $key: skipped (larger than --max-mb)"
    return 0
  fi
  # The data proof is only worth as much as the copy it runs against, so where a
  # recorded sha256 exists the source has to hash to it or the run aborts.
  local expect=""
  if [ -n "$sha_dir" ]; then
    if [ -f "$sha_dir/$key.sha256" ]; then
      expect="$(cut -d' ' -f1 < "$sha_dir/$key.sha256")"
    else
      echo "   !! $key: no $sha_dir/$key.sha256 — running WITHOUT a byte anchor" >&2
    fi
  fi
  echo "-- $key: $src -> $out_dir/$key/$key.rete  (log: $log)"
  # shellcheck disable=SC2086
  if bash "$here/recard.sh" --source "$src" --out "$out_dir/$key/$key.rete" \
       --work "$work" --mode "$mode" --stream-above-mb "$stream_above_mb"        --pyramid-algo "$pyramid_algo" --text-index "$text_index" \
       ${allow_empty:+--allow-empty "$allow_empty"} \
       ${expect:+--expect-sha256 "$expect"} \
       > "$log" 2>&1; then
    printf '%s\tOK\t%s\n' "$key" "$(tail -1 "$log")" >> "$summary"
    echo "   OK"
  else
    printf '%s\tFAILED\t%s\n' "$key" "$(tail -2 "$log" | tr '\n' ' ')" >> "$summary"
    echo "   FAILED — see $log" >&2
    return 1
  fi
}

running=0
while IFS=$'\t' read -r key src size; do
  [ "$src" != "-" ] || continue
  if [ "$jobs" -le 1 ]; then
    if ! run_one "$key" "$src" "$size"; then
      if [ "$stop" = 1 ]; then break; fi
    fi
  else
    # A backgrounded failure cannot set a variable in this shell (its `rc=1`
    # would land in the subshell), so the exit status is read back from the
    # summary file below — the one record both paths write.
    run_one "$key" "$src" "$size" &
    running=$((running + 1))
    if [ $((running % jobs)) -eq 0 ]; then wait; fi
  fi
done < "$plan"
wait

echo
echo "== summary ($summary)"
column -t -s $'\t' "$summary" 2>/dev/null || cat "$summary"
awk -F'\t' '{c[$2]++} END {for (k in c) printf "   %-8s %d\n", k, c[k]}' "$summary"
failed=$(awk -F'\t' '$2 == "FAILED"' "$summary" | wc -l)
[ "$failed" -eq 0 ] || echo "== $failed dataset(s) FAILED — the originals are untouched" >&2
[ "$failed" -eq 0 ]
