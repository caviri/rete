#!/usr/bin/env bash
# Bring ONE already-published .rete up to the current Dataset Card, without
# touching its data.
#
# Why this is not a one-liner: `rete repyramid --card` re-derives the whole card
# by reading the .rete itself (no source RDF needed), but it takes the CURATED
# half only from flags or --card-file — a bare --card silently drops the
# publisher's title/source/license. So the old card has to be read first and
# handed back in. And because the card sits inside the blake3 content hash there
# is no in-place byte swap: the file is rewritten, which is exactly why the data
# has to be proven unchanged afterwards.
#
#   1. read the existing card (2 range reads, cheap even on a 30 GB file)
#   2. carry its curated fields into a --card-file document
#   3. rebuild:  repyramid   (fast, reads the whole file into RAM), or
#                stream      (export -> N-Quads -> build, the two-pass assembler)
#   4. PROVE the data is unchanged: N-Quads of both files must match
#   5. PROVE the new starter queries answer: measured row counts, all non-zero
#   6. only then move the rebuilt file into place, and write a receipt
#
# Any failure in 4 or 5 aborts and leaves the original untouched. The receipt
# makes re-runs idempotent (see recard_batch.sh for the catalog-scale driver).
#
# Everything runs in the rete dev image; the script re-executes itself inside
# Docker, so paths are CONTAINER paths (the repo is at /work).
#
# Usage:
#   bash scripts/recard/recard.sh --source <url|/work/path.rete> --out /work/path.rete [options]
#
# Options:
#   --mode auto|repyramid|stream   rebuild engine (default auto, by file size)
#   --stream-above-mb N            auto switches to stream above N MB (default 192)
#   --work DIR                     scratch + receipts (default /work/dev/recard)
#   --pyramid-algo louvain|types   passed through (default louvain)
#   --allow-empty "id1 id2"        starter queries permitted to return 0 rows
#   --keep                         keep intermediates (.nq, downloads)
#   --force                        redo even if the receipt says it is done
#   --no-verify-data               skip step 4 (NOT recommended; see README)
#
# Env:
#   RETE_IMAGE   docker image           (default rete-dev:latest)
#   RETE_BIN     rete binary in image   (default /work/target/release/rete)
#   RECARD_MOUNT extra "host:container" mount, repeatable via ';'
set -euo pipefail

# ---------------------------------------------------------------- host side --
if [ "${RECARD_INNER:-}" != "1" ]; then
  export MSYS_NO_PATHCONV=1   # Windows Git-Bash: keep /work from being rewritten
  repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  image="${RETE_IMAGE:-rete-dev:latest}"
  if ! docker image inspect "$image" >/dev/null 2>&1; then
    echo "recard: docker image '$image' not found — build it with:" >&2
    echo "        docker compose build dev" >&2
    exit 127
  fi
  mounts=(-v "$repo:/work")
  if [ -n "${RECARD_MOUNT:-}" ]; then
    IFS=';' read -r -a extra <<<"$RECARD_MOUNT"
    for m in "${extra[@]}"; do if [ -n "$m" ]; then mounts+=(-v "$m"); fi; done
  fi
  exec docker run --rm "${mounts[@]}" -w /work \
    -e RECARD_INNER=1 -e RETE_BIN -e TZ=UTC \
    "$image" bash /work/scripts/recard/recard.sh "$@"
fi

# --------------------------------------------------------------- inner side --
RETE="${RETE_BIN:-/work/target/release/rete}"
TOOLS=/work/scripts/recard/card_tools.py
TOOL_VERSION=1

source=""; out=""; mode="auto"; stream_above_mb=192
work="/work/dev/recard"; pyramid_algo="louvain"; allow_empty=""
keep=0; force=0; verify_data=1

die() { echo "recard: $*" >&2; exit 1; }
say() { echo "== $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --source) source="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --mode) mode="$2"; shift 2 ;;
    --stream-above-mb) stream_above_mb="$2"; shift 2 ;;
    --work) work="$2"; shift 2 ;;
    --pyramid-algo) pyramid_algo="$2"; shift 2 ;;
    --allow-empty) allow_empty="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    --force) force=1; shift ;;
    --no-verify-data) verify_data=0; shift ;;
    -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done
[ -n "$source" ] || die "--source is required"
[ -n "$out" ] || die "--out is required"
[ -x "$RETE" ] || die "rete binary not found at $RETE (set RETE_BIN; build with 'docker compose run --rm dev cargo build --release -p rete-cli')"

name="$(basename "$out" .rete)"
scratch="$work/work/$name"
state="$work/state/$name.json"
mkdir -p "$scratch" "$work/state" "$(dirname "$out")"

# The header's blake3-16 content hash identifies a .rete exactly, and reading it
# costs one or two range reads — never a re-hash of a multi-GB file.
content_hash_of() {
  local h
  h="$("$RETE" card "$1" 2>/dev/null | awk '/^  checksum/ {print $3; exit}')"
  if [ -z "$h" ]; then
    # A cardless file has no checksum line, so take the same 16 header bytes
    # directly (still one range read — never a re-hash of the data).
    h="$("$RETE" info "$1" 2>/dev/null | awk '/content_hash: \[/{g=1;next} g&&/\]/{g=0} g{gsub(/[ ,]/,"");printf "%s.",$0}')"
  fi
  printf '%s' "$h"
}
receipt_field() {
  python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get(sys.argv[2],""))' "$state" "$1"
}

# ---- 0. resume, BEFORE spending any bandwidth --------------------------------
# At catalog scale the download is the bill, so the receipt is consulted first:
# same source, output still present and still the file we wrote -> nothing to do.
# For a URL the remote's Content-Length is compared too (one HEAD), which catches
# a source that was republished since.
if [ "$force" != 1 ] && [ -f "$state" ] && [ -f "$out" ] \
   && [ "$(receipt_field source)" = "$source" ] \
   && [ "$(receipt_field output_hash)" = "$(content_hash_of "$out")" ]; then
  fresh=1
  case "$source" in
    http://*|https://*)
      remote_len="$(curl -fsSLI "$source" 2>/dev/null | tr -d '\r' \
                    | awk 'tolower($1)=="content-length:"{n=$2} END{print n}')"
      if [ -n "$remote_len" ] && [ "$remote_len" != "$(receipt_field source_bytes)" ]; then
        fresh=0
        say "the published source changed size ($remote_len bytes) — redoing"
      fi
      ;;
  esac
  if [ "$fresh" = 1 ]; then
    say "already re-carded (receipt $state) — nothing to do"
    exit 0
  fi
fi

# ---- 0b. the local copy ------------------------------------------------------
# A remote source is downloaded ONCE (resumably, curl -C -): the operation is a
# rewrite, so the bytes have to come down whatever we do.
case "$source" in
  http://*|https://*)
    local_src="$scratch/source.rete"
    say "[0/6] fetch $source"
    curl -fSL --retry 3 --retry-delay 2 -C - -o "$local_src" "$source"
    ;;
  *)
    local_src="$source"
    [ -f "$local_src" ] || die "source not found: $local_src"
    ;;
esac
src_bytes=$(stat -c %s "$local_src")
src_hash="$(content_hash_of "$local_src")"
[ -n "$src_hash" ] || src_hash="(unknown:$src_bytes)"
say "source: $local_src ($src_bytes bytes, $src_hash)"

# A local source already at the recorded hash needs no work either.
if [ "$force" != 1 ] && [ -f "$state" ] && [ -f "$out" ] \
   && [ "$(receipt_field source_hash)" = "$src_hash" ] \
   && [ "$(receipt_field output_hash)" = "$(content_hash_of "$out")" ]; then
  say "already re-carded (receipt $state) — nothing to do"
  exit 0
fi

# ---- 1..2. the old card, and its curated half --------------------------------
say "[1/6] read the existing card"
"$RETE" card "$local_src" --json > "$scratch/old-card.json" 2>/dev/null \
  || echo '(no dataset card)' > "$scratch/old-card.json"
say "[2/6] carry the curated fields"
python3 -P "$TOOLS" curated "$scratch/old-card.json" -o "$scratch/curated.json"

# ---- 3. rebuild --------------------------------------------------------------
# repyramid reads the whole file into RAM and holds every quad as strings: peak
# RSS measured at ~20x the file size (see README). Above the threshold the
# two-pass streaming assembler is used instead, at the price of an N-Quads
# staging file on disk.
if [ "$mode" = auto ]; then
  if [ "$src_bytes" -gt $((stream_above_mb * 1024 * 1024)) ]; then mode=stream; else mode=repyramid; fi
fi
tmp_out="$scratch/rebuilt.rete"
rm -f "$tmp_out"
say "[3/6] rebuild ($mode, pyramid-algo=$pyramid_algo)"
case "$mode" in
  repyramid)
    "$RETE" repyramid "$local_src" -o "$tmp_out" \
      --card-file "$scratch/curated.json" --pyramid-algo "$pyramid_algo"
    ;;
  stream)
    # `export --format nq` streams (dump_each) in constant memory; `build` on an
    # .nq FILE takes the two-pass assembler, which never materializes the string
    # quad multiset. The file is needed because that assembler reads its input
    # twice — a pipe cannot be rewound.
    nq="$scratch/staged.nq"
    say "      export -> $nq (needs roughly 10x the .rete in free disk)"
    "$RETE" export "$local_src" --format nq > "$nq"
    say "      staged $(stat -c %s "$nq") bytes"
    "$RETE" build "$nq" -o "$tmp_out" \
      --card-file "$scratch/curated.json" --pyramid-algo "$pyramid_algo"
    [ "$keep" = 1 ] || rm -f "$nq"
    ;;
  *) die "unknown --mode: $mode" ;;
esac
[ -s "$tmp_out" ] || die "rebuild produced nothing"

# ---- 4. the data must be unchanged -------------------------------------------
# Fast path: both files are serialized to N-Quads and hashed as they stream, so
# this costs no disk and constant memory. Byte-equal streams prove equal data.
# Only if the streams differ (a reordering) do we pay for the sorted comparison.
if [ "$verify_data" = 1 ]; then
  say "[4/6] verify the data is unchanged (N-Quads)"
  a=$("$RETE" export "$local_src" --format nq | sha256sum | cut -d' ' -f1)
  b=$("$RETE" export "$tmp_out"   --format nq | sha256sum | cut -d' ' -f1)
  if [ "$a" = "$b" ]; then
    say "      identical N-Quads streams ($a)"
  else
    say "      streams differ in ORDER — falling back to the sorted comparison"
    "$RETE" export "$local_src" --format nq | LC_ALL=C sort -T "$scratch" > "$scratch/a.nq"
    "$RETE" export "$tmp_out"   --format nq | LC_ALL=C sort -T "$scratch" > "$scratch/b.nq"
    if ! cmp "$scratch/a.nq" "$scratch/b.nq"; then
      die "DATA CHANGED — refusing to replace $out (rebuilt file left at $tmp_out)"
    fi
    say "      sorted N-Quads identical ($(wc -l < "$scratch/a.nq") statements)"
    [ "$keep" = 1 ] || rm -f "$scratch/a.nq" "$scratch/b.nq"
  fi
else
  say "[4/6] SKIPPED data verification (--no-verify-data)"
fi

# ---- 5. the new starter queries must answer ----------------------------------
say "[5/6] verify the new card"
"$RETE" card "$tmp_out" --json > "$scratch/new-card.json"
# shellcheck disable=SC2086
python3 -P "$TOOLS" verify --old "$scratch/old-card.json" --new "$scratch/new-card.json" \
  ${allow_empty:+--allow-empty $allow_empty} \
  || die "the rebuilt card did not pass — $out left untouched (rebuilt file at $tmp_out)"

# ---- 6. install + receipt ----------------------------------------------------
# Two moves: the first may cross filesystems, the second is a rename within one
# directory and is therefore atomic — a reader never sees a half-written file.
say "[6/6] install -> $out"
mv -f "$tmp_out" "$out.recard-new"
mv -f "$out.recard-new" "$out"
out_hash="$(content_hash_of "$out")"
# Every value arrives as argv, never interpolated into the program text: a
# source URL or a mode string is data, not code.
python3 -P - "$state" "$scratch" "$source" "$src_hash" "$src_bytes" "$out" \
    "$out_hash" "$mode" "$pyramid_algo" "$verify_data" "$TOOL_VERSION" <<'PY'
import datetime, json, os, sys
(state, scratch, source, src_hash, src_bytes, out, out_hash, mode,
 pyramid_algo, verify_data, tool_version) = sys.argv[1:12]
new = json.load(open(os.path.join(scratch, "new-card.json")))
rows = {q["id"]: q.get("rows", 0)
        for q in ((new.get("build") or {}).get("query_costs") or {}).get("queries", [])}
json.dump({
    "tool_version": int(tool_version),
    "source": source,
    "source_hash": src_hash,
    "source_bytes": int(src_bytes),
    "output": out,
    "output_hash": out_hash,
    "output_bytes": os.path.getsize(out),
    "mode": mode,
    "pyramid_algo": pyramid_algo,
    "data_verified": verify_data == "1",
    "curated_fields": sorted(json.load(open(os.path.join(scratch, "curated.json")))),
    "starter_query_rows": rows,
    "recarded_at": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0).isoformat().replace("+00:00", "Z"),
}, open(state, "w"), indent=2, sort_keys=True)
print("receipt ->", state)
PY

if [ "$keep" != 1 ]; then
  rm -f "$scratch/source.rete" 2>/dev/null || true
fi
say "done: $out ($out_hash)"
