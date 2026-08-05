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
#   --pyramid-algo auto|louvain|types
#                                  auto (default) reproduces what the SOURCE has:
#                                  a one-level pyramid is `types`, anything
#                                  deeper is `louvain`. `repyramid`'s own default
#                                  is louvain, which is the wrong answer for 17 of
#                                  the 22 published files audited on 2026-08-05 —
#                                  see README "Known limits".
#   --allow-empty "id1 id2"        starter queries permitted to return 0 rows
#   --text-index auto|yes|no       auto (default) mirrors the source: a file that
#                                  has a full-text index keeps one, because a
#                                  rebuild derives from the QUADS and would
#                                  otherwise drop it silently
#   --allow-section-loss           proceed even though the source carries a
#                                  section the rebuild cannot reproduce
#   --expect-sha256 HEX            abort unless the source's bytes hash to HEX —
#                                  anchors the data proof to the PUBLISHED file
#                                  rather than to whatever copy happened to be
#                                  on disk. The receipt records the hash either
#                                  way, so the proof stays checkable later.
#   --keep                         keep intermediates (.nq, downloads)
#   --reuse-staged                 in --mode stream, reuse an existing staged
#                                  .nq instead of re-exporting. Staging a large
#                                  file is the long half (gbif-birds: 43.4 GiB
#                                  in 28 minutes), and a build that dies after
#                                  it should not cost it twice. Only meaningful
#                                  with --keep on the run that produced it.
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
TOOL_VERSION=2

source=""; out=""; mode="auto"; stream_above_mb=192
work="/work/dev/recard"; pyramid_algo="auto"; allow_empty=""; expect_sha=""
keep=0; force=0; verify_data=1; text_index="auto"; allow_section_loss=0
reuse_staged=0

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
    --expect-sha256) expect_sha="$2"; shift 2 ;;
    --text-index) text_index="$2"; shift 2 ;;
    --allow-section-loss) allow_section_loss=1; shift ;;
    --keep) keep=1; shift ;;
    --reuse-staged) reuse_staged=1; shift ;;
    --force) force=1; shift ;;
    --no-verify-data) verify_data=0; shift ;;
    -h|--help) sed -n '2,68p' "$0"; exit 0 ;;
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

# Peak memory, because the rebuild engine is chosen by a RAM prediction and the
# README's table is the only thing that makes that prediction checkable. cgroup
# v2 exposes the container's high-water mark; there is no `/usr/bin/time` in the
# image. It is a CONTAINER peak (page cache from reading a large staged .nq
# counts), so it is an upper bound on the process's VmHWM, not equal to it.
PEAK_FILE=/sys/fs/cgroup/memory.peak
peak_reset() { [ -w "$PEAK_FILE" ] && echo 0 > "$PEAK_FILE" 2>/dev/null; return 0; }
peak_read() { [ -r "$PEAK_FILE" ] && cat "$PEAK_FILE" 2>/dev/null || echo ""; }
human_bytes() {
  [ -n "$1" ] || { printf 'unknown'; return; }
  awk -v b="$1" 'BEGIN{ if (b>=1073741824) printf "%.2f GiB", b/1073741824;
                        else printf "%.0f MiB", b/1048576 }'
}

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

# A rebuild re-derives the file from its QUADS. Anything else the source carries
# — the full-text index, an alien section such as the PMTiles archive embedded in
# `geoadmin-tiles` — is not in the quads and does not come back on its own. The
# N-Quads proof cannot see that, because it compares the RDF and nothing else. So
# the loss is detected here, before hours of work, rather than discovered by a
# reader whose CONTAINS query got slow or whose map went blank.
header_field() { "$RETE" info "$1" 2>/dev/null | awk -F'[ ,]+' -v k="$2" '$2 == k":" {print $3; exit}'; }
alien_sections() {
  # Section kinds a rebuild cannot reproduce. Kind 7 is BuildInfo, which a
  # rebuild writes fresh — unless its payload is not JSON, in which case
  # something else claimed the kind (see scripts/embed_tiles.py) and rebuilding
  # destroys it.
  python3 -P - "$1" <<'PY'
import struct, sys
path = sys.argv[1]
with open(path, "rb") as fh:
    head = fh.read(1024)
    n = struct.unpack_from("<H", head, 44)[0]
    for i in range(n):
        kind, = struct.unpack_from("<H", head, 64 + i * 24)
        off, ln = struct.unpack_from("<QQ", head, 64 + i * 24 + 8)
        if kind == 7 and ln:
            fh.seek(off)
            if not fh.read(1).startswith(b"{"):
                print(f"kind-7 section of {ln} bytes whose payload is not build-info JSON")
        elif kind > 7 and ln:
            print(f"unknown kind-{kind} section of {ln} bytes")
PY
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
# The header content hash identifies the IMAGE; the file sha256 identifies the
# BYTES. Only the second one can say "this is the object that is published" — a
# rebuilt local copy can carry the same content hash as the published file while
# differing in the unhashed build-info section. The data proof in step 4 is only
# worth as much as the copy it runs against, so anchor it here and record it in
# the receipt whether or not --expect-sha256 was passed.
src_sha256="$(sha256sum "$local_src" | cut -d' ' -f1)"
say "source: $local_src ($src_bytes bytes, $src_hash, sha256 $src_sha256)"
if [ -n "$expect_sha" ] && [ "$expect_sha" != "$src_sha256" ]; then
  die "source sha256 mismatch: expected $expect_sha, got $src_sha256 — this is NOT the file you meant to re-card"
fi

# A local source already at the recorded hash needs no work either.
if [ "$force" != 1 ] && [ -f "$state" ] && [ -f "$out" ] \
   && [ "$(receipt_field source_hash)" = "$src_hash" ] \
   && [ "$(receipt_field output_hash)" = "$(content_hash_of "$out")" ]; then
  say "already re-carded (receipt $state) — nothing to do"
  exit 0
fi

# ---- 1..2. the old card, and its curated half --------------------------------
# ---- 0c. what else is in this file that a rebuild would not put back? --------
# The community algorithm is not recorded anywhere in an older file, but the
# pyramid it produced is: `types` yields exactly ONE level, `louvain` three to
# six. That is enough to reproduce the source's own structure instead of
# imposing `repyramid`'s default on it.
if [ "$pyramid_algo" = auto ]; then
  src_levels="$(header_field "$local_src" pyramid_levels)"
  : "${src_levels:=0}"
  if [ "$src_levels" = 1 ]; then pyramid_algo=types; else pyramid_algo=louvain; fi
  say "pyramid-algo auto -> $pyramid_algo (source has $src_levels level(s))"
fi

src_text_index="$(header_field "$local_src" text_index_len)"
: "${src_text_index:=0}"
case "$text_index" in
  auto) if [ "$src_text_index" -gt 0 ]; then want_text_index=1; else want_text_index=0; fi ;;
  yes|1|true) want_text_index=1 ;;
  no|0|false) want_text_index=0 ;;
  *) die "--text-index takes auto|yes|no" ;;
esac
if [ "$src_text_index" -gt 0 ]; then
  if [ "$want_text_index" = 1 ]; then
    say "source carries a $src_text_index-byte full-text index — rebuilding one (--text-index)"
  else
    say "WARNING: dropping the source's $src_text_index-byte full-text index (--text-index no)"
  fi
fi
alien="$(alien_sections "$local_src")"
if [ -n "$alien" ]; then
  echo "recard: $local_src carries a section a rebuild cannot reproduce:" >&2
  echo "$alien" | sed 's/^/  - /' >&2
  if [ "$allow_section_loss" != 1 ]; then
    die "refusing to rebuild and silently drop it (pass --allow-section-loss to accept, then re-attach it yourself)"
  fi
  say "proceeding anyway (--allow-section-loss) — the section will NOT be in the output"
fi

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
say "[3/6] rebuild ($mode, pyramid-algo=$pyramid_algo, text-index=$want_text_index)"
staged_bytes=""
ti_flag=()
[ "$want_text_index" = 1 ] && ti_flag=(--text-index)
peak_reset
case "$mode" in
  repyramid)
    "$RETE" repyramid "$local_src" -o "$tmp_out" "${ti_flag[@]}" \
      --card-file "$scratch/curated.json" --pyramid-algo "$pyramid_algo"
    ;;
  stream)
    # `export --format nq` streams (dump_each) in constant memory; `build` on an
    # .nq FILE takes the two-pass assembler, which never materializes the string
    # quad multiset. The file is needed because that assembler reads its input
    # twice — a pipe cannot be rewound.
    nq="$scratch/staged.nq"
    if [ "$reuse_staged" = 1 ] && [ -s "$nq" ]; then
      staged_bytes=$(stat -c %s "$nq")
      say "      reusing the staged $nq ($(human_bytes "$staged_bytes")) — --reuse-staged"
    else
      # The staged size runs 9-15x the .rete on ordinary graphs and far more on a
      # dense one (gbif-birds: 1.53 GB -> 43.38 GiB, 30x), so free disk is what
      # kills a large staged build hours in. Say what is available before
      # spending the hours, not after.
      say "      export -> $nq (9-15x the .rete on ordinary graphs, 30x on a dense one)"
      say "      free on the scratch filesystem: $(df -Pk "$scratch" | awk 'NR==2{printf "%.1f GiB", $4/1048576}')"
      "$RETE" export "$local_src" --format nq > "$nq"
      staged_bytes=$(stat -c %s "$nq")
      say "      staged $staged_bytes bytes ($(human_bytes "$staged_bytes"))"
    fi
    "$RETE" build "$nq" -o "$tmp_out" "${ti_flag[@]}" \
      --card-file "$scratch/curated.json" --pyramid-algo "$pyramid_algo"
    [ "$keep" = 1 ] || rm -f "$nq"
    ;;
  *) die "unknown --mode: $mode" ;;
esac
peak_rebuild="$(peak_read)"
say "      peak container memory during the rebuild: $(human_bytes "$peak_rebuild")"
[ -s "$tmp_out" ] || die "rebuild produced nothing"
out_text_index="$(header_field "$tmp_out" text_index_len)"
: "${out_text_index:=0}"
# The pyramid is DERIVED, so the N-Quads proof says nothing about it — yet it is
# the largest thing a re-card changes, and a wrong `--pyramid-algo` is invisible
# without this comparison. Measured on the 2026-08-05 batch: rebuilding a
# `types` file with `louvain` inflated `arxiu`'s pyramid 726 KB -> 45.7 MB and
# `proteinbase`'s 138 KB -> 5.87 MB, while the right algorithm reproduced the
# published section to within a few bytes. A large ratio is the tell.
src_pyr="$(header_field "$local_src" pyramid_meta_len)"; : "${src_pyr:=0}"
out_pyr="$(header_field "$tmp_out" pyramid_meta_len)"; : "${out_pyr:=0}"
say "      pyramid: $src_pyr B -> $out_pyr B (algo $pyramid_algo)"
if [ "$src_pyr" -gt 0 ] && [ "$out_pyr" -gt $((src_pyr * 4)) ]; then
  other=types; [ "$pyramid_algo" = types ] && other=louvain
  say "      WARNING: the rebuilt pyramid is ${out_pyr}B against the source's ${src_pyr}B."
  say "               That usually means the file was built with --pyramid-algo $other."
fi
if [ "$want_text_index" = 1 ] && [ "$out_text_index" = 0 ]; then
  die "the rebuild produced no full-text index although the source had $src_text_index bytes of one — $out left untouched"
fi
[ "$out_text_index" -gt 0 ] && say "      full-text index: $src_text_index B -> $out_text_index B"

# ---- 4. the data must be unchanged -------------------------------------------
# Fast path: both files are serialized to N-Quads and hashed as they stream, so
# this costs no disk and constant memory. Byte-equal streams prove equal data.
# Only if the streams differ (a reordering) do we pay for the sorted comparison.
data_proof="skipped"; nq_sha256=""; nq_statements=""
if [ "$verify_data" = 1 ]; then
  say "[4/6] verify the data is unchanged (N-Quads)"
  a=$("$RETE" export "$local_src" --format nq | sha256sum | cut -d' ' -f1)
  b=$("$RETE" export "$tmp_out"   --format nq | sha256sum | cut -d' ' -f1)
  if [ "$a" = "$b" ]; then
    say "      identical N-Quads streams ($a)"
    data_proof="stream-identical"; nq_sha256="$a"
  else
    say "      streams differ in ORDER — falling back to the sorted comparison"
    "$RETE" export "$local_src" --format nq | LC_ALL=C sort -T "$scratch" > "$scratch/a.nq"
    "$RETE" export "$tmp_out"   --format nq | LC_ALL=C sort -T "$scratch" > "$scratch/b.nq"
    if ! cmp "$scratch/a.nq" "$scratch/b.nq"; then
      die "DATA CHANGED — refusing to replace $out (rebuilt file left at $tmp_out)"
    fi
    nq_statements="$(wc -l < "$scratch/a.nq")"
    nq_sha256="$(sha256sum "$scratch/a.nq" | cut -d' ' -f1)"
    say "      sorted N-Quads identical ($nq_statements statements, sorted sha256 $nq_sha256)"
    data_proof="sorted-cmp"
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
    "$out_hash" "$mode" "$pyramid_algo" "$verify_data" "$TOOL_VERSION" \
    "$src_sha256" "$data_proof" "$nq_sha256" "$nq_statements" \
    "${peak_rebuild:-}" "${staged_bytes:-}" "${src_text_index:-0}" "${out_text_index:-0}" <<'PY'
import datetime, json, os, sys
(state, scratch, source, src_hash, src_bytes, out, out_hash, mode,
 pyramid_algo, verify_data, tool_version, src_sha256, data_proof,
 nq_sha256, nq_statements, peak_rebuild, staged_bytes,
 src_text_index, out_text_index) = sys.argv[1:20]
old = json.load(open(os.path.join(scratch, "old-card.json"))) \
    if os.path.getsize(os.path.join(scratch, "old-card.json")) > 40 else {}
new = json.load(open(os.path.join(scratch, "new-card.json")))
rows = {q["id"]: q.get("rows", 0)
        for q in ((new.get("build") or {}).get("query_costs") or {}).get("queries", [])}
json.dump({
    "tool_version": int(tool_version),
    "source": source,
    "source_hash": src_hash,
    # The bytes that were actually re-carded. Together with `data_proof` this is
    # the whole audit trail: THIS object, exported to N-Quads, equals the one the
    # rebuilt file exports.
    "source_sha256": src_sha256,
    "source_bytes": int(src_bytes),
    "output": out,
    "output_hash": out_hash,
    "output_bytes": os.path.getsize(out),
    "mode": mode,
    "pyramid_algo": pyramid_algo,
    # A container high-water mark, so it is an upper bound on the process's
    # VmHWM (it includes page cache from reading the staged .nq).
    "rebuild_peak_bytes": int(peak_rebuild) if peak_rebuild else None,
    "staged_nquads_bytes": int(staged_bytes) if staged_bytes else None,
    # A rebuild derives from the quads; the text index is not in the quads. The
    # N-Quads proof cannot see it going missing, so record both sides here.
    "text_index_bytes_before": int(src_text_index),
    "text_index_bytes_after": int(out_text_index),
    "data_verified": verify_data == "1",
    "data_proof": data_proof,
    "nquads_sha256": nq_sha256,
    "nquads_statements": int(nq_statements) if nq_statements else None,
    "curated_fields": sorted(json.load(open(os.path.join(scratch, "curated.json")))),
    # Old vs new, so a shorter query list can be read as the repair it is rather
    # than as a loss, and so a shrinking count is visible without re-reading the
    # cards. Old cards counted pre-dedup input; the header counts the file.
    "queries_before": [q["id"] for q in (old.get("queries") or [])],
    "queries_after": [q["id"] for q in (new.get("queries") or [])],
    "dropped_queries": [{"id": d.get("id"), "why": d.get("why")}
                        for d in ((new.get("build") or {}).get("dropped_queries") or [])],
    "counts_before": {k: old.get(k) for k in
                      ("triple_count", "quad_count", "named_graph_count")},
    "counts_after": {k: new.get(k) for k in
                     ("triple_count", "quad_count", "named_graph_count")},
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
