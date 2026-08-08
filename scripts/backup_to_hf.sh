#!/usr/bin/env bash
# Back up published .rete bytes to the durable Hugging Face bucket.
#
# WHY TWO STORES, AND WHY THEY ARE NOT INTERCHANGEABLE
#   R2 *serves*. https://data.graphplaza.com/<key> answers an HTTP Range request
#   directly -- 206 + Content-Range, no redirect -- which is the only thing the
#   playground's default synchronous-XHR worker reader can consume, and it
#   benchmarks ~3.5x faster than the HF Space.
#   An HF bucket *cannot* serve that reader at all. It is Xet content-addressed
#   chunk storage: the public resolve URL works, but 302-redirects to a
#   per-range-signed Xet-bridge CDN, and a synchronous XHR cannot follow a 302.
#   The CLI is happy; the browser is not.
# So this is a BACKUP, never a second origin. R2 stays the only published URL.
#
#   scripts/backup_to_hf.sh data/foo/foo.rete      # one dataset, after publish
#   scripts/backup_to_hf.sh --source both          # whole corpus sweep
#   scripts/backup_to_hf.sh --source both --dry-run
#
# Destination keys mirror the R2 layout exactly, under one prefix:
#   local data/<dataset>/<file>.rete  ==  R2 <dataset>/<file>.rete
#                                     ->  hf   rete/<dataset>/<file>.rete
#
# Resumable: an object already in the bucket at the identical byte size is
# skipped, so a killed run is restarted by re-running it.
#
# Options:
#   --source local|r2|both  what to enumerate when no path is given. local =
#                           every data/**/*.rete; r2 = every .rete object in the
#                           serving bucket. Default: local (or none, when paths
#                           are given).
#   --dry-run               plan and verify only; upload nothing. Exits non-zero
#                           if any source file is unsound.
#   --all-objects           with --source r2, mirror every object, not just .rete.
#   --bucket NS/NAME        destination bucket (default $RETE_HF_BUCKET).
#   --prefix P              destination prefix (default rete).
#   --r2-bucket NAME        source R2 bucket (default $RETE_BUCKET).
#   --margin-gb N           free space to preserve while staging (default 25).
#
# Env:
#   RETE_HF_BUCKET   destination bucket   (default katospiegel/rete-public)
#   RETE_BUCKET      source R2 bucket     (default rete)
#   ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT, or the repo .env
#     -- only needed for --source r2/both.
#
# Host tools: `hf` (the Hugging Face CLI; the one host exception in this repo).
# Everything else -- the R2 listing and any streaming download -- runs in a
# container.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUCKET="${RETE_HF_BUCKET:-katospiegel/rete-public}"
R2_BUCKET="${RETE_BUCKET:-rete}"
PREFIX="rete"
STATE="$ROOT/dev/backup-hf"
STAGE="$STATE/stage"
LOG="$STATE/backup.log"
MARGIN_GB=25
SOURCE=""
DRY_RUN=0
ALL_OBJECTS=0
PATHS=()

# The header comment above IS the help text: print it up to the first line that
# is not a comment, so the two can never drift apart.
usage() {
  awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "$0"
  exit "${1:-0}"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --source)     SOURCE="${2:?--source needs local|r2|both}"; shift 2 ;;
    --prefix)     PREFIX="${2:?--prefix needs a value}"; shift 2 ;;
    --bucket)     BUCKET="${2:?--bucket needs ns/name}"; shift 2 ;;
    --r2-bucket)  R2_BUCKET="${2:?--r2-bucket needs a name}"; shift 2 ;;
    --margin-gb)  MARGIN_GB="${2:?--margin-gb needs a number}"; shift 2 ;;
    --dry-run)    DRY_RUN=1; shift ;;
    --all-objects) ALL_OBJECTS=1; shift ;;
    -h|--help)    usage 0 ;;
    --) shift; while [ $# -gt 0 ]; do PATHS+=("$1"); shift; done ;;
    -*) echo "unknown option: $1" >&2; usage 2 ;;
    *)  PATHS+=("$1"); shift ;;
  esac
done

if [ ${#PATHS[@]} -eq 0 ] && [ -z "$SOURCE" ]; then SOURCE="local"; fi
case "${SOURCE:-}" in ""|local|r2|both) ;; *) echo "bad --source: $SOURCE" >&2; exit 2 ;; esac

mkdir -p "$STATE" "$STAGE"

# ---------------------------------------------------------------------------
# Single-instance lock.
#
# Not paranoia: two copies of the sweep each passed their own free-space check
# and then jointly exhausted the disk -- two concurrent 56 GB pulls took 68 GB
# before being killed, and one of them left a zero-byte file behind. Every
# free-space decision below is only sound while exactly one instance is making
# them, and `mkdir` is the atomic test-and-set that guarantees that.
# ---------------------------------------------------------------------------
LOCK="$STATE/.lock"
if ! mkdir "$LOCK" 2>/dev/null; then
  echo "another backup_to_hf.sh holds $LOCK (pid $(cat "$LOCK/pid" 2>/dev/null || echo '?')) -- refusing to start" >&2
  exit 3
fi
echo "$$" > "$LOCK/pid"
cleanup() { rm -f "$LOCK/pid"; rmdir "$LOCK" 2>/dev/null; rm -f "$STAGE"/*; }
trap cleanup EXIT INT TERM

# R2 credentials for the aws-cli container: whatever is exported, plus the
# repository's gitignored .env when it exists (same contract as upload_bucket.sh).
R2_ENV=()
[ -n "${ACCESS_KEY_ID:-}" ]     && R2_ENV+=(-e ACCESS_KEY_ID)
[ -n "${SECRET_ACCESS_KEY:-}" ] && R2_ENV+=(-e SECRET_ACCESS_KEY)
[ -n "${S3_API_ENDPOINT:-}" ]   && R2_ENV+=(-e S3_API_ENDPOINT)
[ -f "$ROOT/.env" ]             && R2_ENV+=(--env-file "$ROOT/.env")

log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" >> "$LOG"; }
say() { printf '%s\n' "$*"; log "$*"; }
gb()  { awk -v b="${1:-0}" 'BEGIN{printf "%.2f", b/1073741824}'; }
size_of() { stat -c %s "$1" 2>/dev/null || echo 0; }

say "=== $(date -u +%FT%TZ) backup -> hf://buckets/$BUCKET/$PREFIX/ (dry-run=$DRY_RUN) ==="

# `hf buckets ls --json` emits one object per file with a stable field order;
# pairing "path" with the following "size" beats splitting the human table on
# whitespace, which breaks on any key containing a space.
hf_list() {
  hf buckets ls "$BUCKET/${1:-}" --recursive --json 2>/dev/null \
    | awk -F'"' '/"path":/{p=$4} /"size":/{s=$0; gsub(/[^0-9]/,"",s); print s"\t"p}'
}

# Byte size of ONE key in the bucket, or empty. The prefix match is a plain
# string match (rete/openalex also matches rete/openalex-astrocytes), so the
# key is compared for equality.
hf_size() {
  hf_list "$1" | awk -F'\t' -v k="$1" '$2==k {print $1; exit}'
}

STATE_TSV="$STATE/hf_state.tsv"
say "snapshotting the bucket ..."
hf_list "$PREFIX" > "$STATE_TSV"
rc=$?
if [ "$rc" -ne 0 ] || [ ! -s "$STATE_TSV" ]; then
  # An empty listing is legitimate for a first run, but an `hf` that is missing,
  # unauthenticated or offline also lists nothing -- and would make every object
  # look absent and get re-uploaded. Prove the CLI answers before trusting it.
  if ! hf buckets ls "$BUCKET" --recursive --json >/dev/null 2>&1; then
    echo "cannot list $BUCKET (hf missing, not logged in, or offline)" >&2
    exit 4
  fi
fi
say "bucket holds $(wc -l < "$STATE_TSV") object(s) under $PREFIX/"

have_size() { awk -F'\t' -v k="$1" '$2==k {print $1; exit}' "$STATE_TSV"; }

# ---------------------------------------------------------------------------
# The work list: size <TAB> destination key <TAB> local path (or "-" for an
# object that lives only on R2). Largest first, so an interruption leaves the
# most valuable bytes already protected.
# ---------------------------------------------------------------------------
TODO="$STATE/todo.tsv"
: > "$TODO"

key_for() {  # path -> R2/HF key, i.e. everything below data/
  case "$1" in
    data/*)   printf '%s\n' "${1#data/}" ;;
    */data/*) printf '%s\n' "${1##*/data/}" ;;
    # Outside a data/ tree, fall back to upload_r2.py's default key shape.
    *) base="${1##*/}"; printf '%s/%s\n' "${base%.*}" "$base" ;;
  esac
}

add_local() {
  local abs="$1" rel
  case "$abs" in "$ROOT"/*) rel="${abs#"$ROOT"/}" ;; *) rel="$abs" ;; esac
  printf '%s\t%s/%s\t%s\n' "$(size_of "$abs")" "$PREFIX" "$(key_for "$rel")" "$abs" >> "$TODO.raw"
}

: > "$TODO.raw"
for p in ${PATHS+"${PATHS[@]}"}; do
  if [ -d "$p" ]; then
    while IFS= read -r f; do add_local "$(cd "$(dirname "$f")" && pwd)/$(basename "$f")"; done \
      < <(find "$p" -type f)
  elif [ -f "$p" ]; then
    add_local "$(cd "$(dirname "$p")" && pwd)/$(basename "$p")"
  else
    say "MISSING $p"; echo "$p (missing)" >> "$STATE/failures.txt"
  fi
done

if [ "$SOURCE" = "local" ] || [ "$SOURCE" = "both" ]; then
  while IFS= read -r f; do add_local "$ROOT/${f#./}"; done \
    < <(cd "$ROOT" && find data -type f -name '*.rete' 2>/dev/null)
fi

if [ "$SOURCE" = "r2" ] || [ "$SOURCE" = "both" ]; then
  say "listing s3://$R2_BUCKET ..."
  R2_TXT="$STATE/r2.txt"
  MSYS_NO_PATHCONV=1 docker run --rm ${R2_ENV+"${R2_ENV[@]}"} \
    --entrypoint bash amazon/aws-cli -c \
    "AWS_ACCESS_KEY_ID=\$ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY=\$SECRET_ACCESS_KEY \
     aws s3 ls s3://$R2_BUCKET/ --recursive --endpoint-url \$S3_API_ENDPOINT" \
    > "$R2_TXT" 2>"$STATE/r2.err"
  rc=$?
  say "aws s3 ls exit=$rc, $(wc -l < "$R2_TXT") object(s)"
  [ "$rc" -ne 0 ] && { say "FATAL cannot list R2: $(tail -1 "$STATE/r2.err")"; exit 5; }
  awk -v pre="$PREFIX" -v all="$ALL_OBJECTS" \
      '(all==1 || $4 ~ /\.rete$/) && $3+0 > 0 {print $3"\t"pre"/"$4"\t-"}' \
      "$R2_TXT" >> "$TODO.raw"
fi

# De-duplicate on the destination key (a file can be both local and on R2;
# keep the row that has a local path, which needs no download) then sort big
# files first.
LC_ALL=C sort -t$'\t' -k2,2 -k3,3r "$TODO.raw" \
  | awk -F'\t' '!seen[$2]++' \
  | LC_ALL=C sort -t$'\t' -k1,1nr > "$TODO"
say "work list: $(wc -l < "$TODO") object(s), $(gb "$(awk -F'\t' '{s+=$1} END{print s+0}' "$TODO")") GB"

# ---------------------------------------------------------------------------
# Pre-upload integrity. A killed download once left a ZERO-BYTE file that a
# naive uploader would have shipped as a perfectly valid-looking, completely
# truncated graph. Size alone is checked for any file; a .rete additionally
# carries the 4-byte `RETE` magic at BOTH ends, so head+tail proves the file is
# whole end to end for the price of 8 bytes.
# ---------------------------------------------------------------------------
sane_file() {
  local f="$1" want="$2" got
  got=$(size_of "$f")
  [ "$got" -gt 0 ] || { say "REJECT $f is zero bytes"; return 1; }
  if [ -n "$want" ] && [ "$want" != "0" ] && [ "$got" != "$want" ]; then
    say "REJECT $f is $got bytes, expected $want"; return 1
  fi
  case "$f" in
    *.rete)
      if [ "$(head -c 4 "$f" 2>/dev/null)" != "RETE" ] \
      || [ "$(tail -c 4 "$f" 2>/dev/null)" != "RETE" ]; then
        say "REJECT $f has no RETE magic at both ends (truncated?)"; return 1
      fi ;;
  esac
  return 0
}

free_gb() { df -Pk "$STAGE" | awk 'NR==2 {printf "%d", $4/1048576}'; }

# Stdout is the staged path and NOTHING else -- the caller reads it through a
# command substitution, so every message in here goes to stderr.
pull_from_r2() {  # key(without prefix) size -> staged path on stdout
  local key="$1" want="$2" base tmp need avail
  base="${key##*/}"; tmp="$STAGE/$base"
  need=$(awk -v b="$want" 'BEGIN{printf "%d", b/1073741824 + 1}')
  avail=$(free_gb)
  if [ "$avail" -lt $((need + MARGIN_GB)) ]; then
    say "NOSPACE $key needs ${need}GB + ${MARGIN_GB}GB margin, have ${avail}GB" >&2; return 1
  fi
  say "PULL  $key (${avail}GB free, staging)" >&2
  MSYS_NO_PATHCONV=1 docker run --rm ${R2_ENV+"${R2_ENV[@]}"} \
    -v "$STAGE:/stage" --entrypoint bash amazon/aws-cli -c \
    "AWS_ACCESS_KEY_ID=\$ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY=\$SECRET_ACCESS_KEY \
     aws s3 cp s3://$R2_BUCKET/$key /stage/$base --endpoint-url \$S3_API_ENDPOINT --only-show-errors" \
    >> "$LOG" 2>&1
  local rc=$?
  # The download's exit code is a hint, not the verdict -- sane_file is.
  if [ "$rc" -ne 0 ]; then say "PULL  exit=$rc for $key (verifying anyway)" >&2; fi
  printf '%s\n' "$tmp"
}

# ---------------------------------------------------------------------------
# Upload, then VERIFY. `hf buckets cp` exiting 0 is not evidence that the object
# landed at the right length; only reading the size back out of the bucket is.
# An upload whose exit code was non-zero but whose bytes verify still counts as
# done, and vice versa.
# ---------------------------------------------------------------------------
up=0; verified=0; skipped=0; failed=0; bytes=0; planned=0; unsound=0
: > "$STATE/failures.txt"

while IFS=$'\t' read -r size dest src; do
  have=$(have_size "$dest")
  if [ -n "$have" ] && [ "$have" = "$size" ]; then
    skipped=$((skipped+1)); log "SKIP  $dest (already $size)"; continue
  fi

  if [ "$DRY_RUN" = "1" ]; then
    planned=$((planned+1)); bytes=$((bytes+size))
    if [ "$src" != "-" ]; then
      if sane_file "$src" "$size"; then
        st="local ok"
      else
        st="local UNSOUND"; unsound=$((unsound+1)); echo "$dest (unsound source)" >> "$STATE/failures.txt"
      fi
    else
      st="stream from r2"
    fi
    say "PLAN  $dest ($(gb "$size") GB, $st)"
    continue
  fi

  staged=""
  if [ "$src" = "-" ]; then
    staged=$(pull_from_r2 "${dest#"$PREFIX"/}" "$size") || {
      failed=$((failed+1)); echo "$dest (no space)" >> "$STATE/failures.txt"; continue; }
    src="$staged"
  fi

  if ! sane_file "$src" "$size"; then
    failed=$((failed+1)); echo "$dest (unsound source)" >> "$STATE/failures.txt"
    [ -n "$staged" ] && rm -f "$staged"
    continue
  fi

  say "PUT   $dest ($(gb "$size") GB) <- $src"
  hf buckets cp "$src" "hf://buckets/$BUCKET/$dest" >> "$LOG" 2>&1
  cp_rc=$?
  [ -n "$staged" ] && rm -f "$staged"   # the disk is the constraint: free it now

  landed=$(hf_size "$dest")
  if [ "$landed" = "$size" ]; then
    up=$((up+1)); bytes=$((bytes+size)); say "OK    $dest (cp exit=$cp_rc, verified $landed bytes)"
  else
    failed=$((failed+1)); say "FAIL  $dest (cp exit=$cp_rc, bucket says '${landed:-absent}', wanted $size)"
    echo "$dest (verify)" >> "$STATE/failures.txt"
  fi
done < "$TODO"

# ---------------------------------------------------------------------------
# Corpus-level verification: re-read the bucket and confirm EVERY object in the
# work list -- including the ones skipped as already-present -- is there at the
# right length. This is the number worth quoting.
# ---------------------------------------------------------------------------
say "re-listing the bucket to verify ..."
hf_list "$PREFIX" > "$STATE/hf_final.tsv"
# `have[$2] "" != $1 ""` forces a STRING compare: awk would otherwise read an
# absent key as the uninitialised 0 and call a zero-byte object "present".
missing=$(awk -F'\t' 'NR==FNR {have[$2]=$1; next}
                      !($2 in have) || have[$2] "" != $1 "" {print $2}' \
  "$STATE/hf_final.tsv" "$TODO")
verified=$(( $(wc -l < "$TODO") - $(printf '%s' "$missing" | grep -c . ) ))
say "verified $verified/$(wc -l < "$TODO") object(s) present at matching byte counts"
if [ -n "$missing" ]; then
  printf '%s\n' "$missing" > "$STATE/missing.txt"
  say "still missing (see $STATE/missing.txt):"
  printf '%s\n' "$missing" | head -20 | sed 's/^/  /'
fi

if [ "$DRY_RUN" = "1" ]; then
  say "DRY RUN: planned=$planned skipped=$skipped unsound=$unsound bytes=$(gb "$bytes") GB -- nothing was uploaded"
  # Non-zero on an unsound source, so --dry-run is usable as a pre-flight gate.
  [ "$unsound" -eq 0 ]
  exit $?
fi
say "uploaded=$up skipped=$skipped failed=$failed bytes=$(gb "$bytes") GB"
[ "$failed" -eq 0 ] && [ -z "$missing" ]
exit $?
