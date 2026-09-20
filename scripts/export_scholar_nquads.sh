#!/usr/bin/env bash
# Export the scholar constellation to gzipped N-Quads and publish it under
# `scholar/` in the public Hugging Face bucket.
#
# WHY THIS EXISTS
#   A `.rete` is queryable over range reads by anything that speaks rete. Nothing
#   else speaks rete. `rete export --format nq` is the lossless bridge (default
#   graph + named graphs + RDF-star), and every triple store bulk-loads N-Quads,
#   so one gzipped dump per dataset makes the whole constellation loadable into
#   Oxigraph / GraphDB / Jena by someone who has never heard of this repo.
#   docs/interop.md documents the load commands this feeds.
#
#   scripts/export_scholar_nquads.sh --all --dry-run     # plan the whole corpus
#   scripts/export_scholar_nquads.sh ror                 # one dataset by NAME
#   scripts/export_scholar_nquads.sh --all               # sweep
#   scripts/export_scholar_nquads.sh --all --audit-only  # count invalid IRIs, upload nothing
#
# THE LOOP — strictly sequential, one dataset at a time, self-cleaning
#
#   1. locate the .rete: a local copy under --data at the published byte size,
#      else download it (resumable, `curl -C -`)
#   2. /usr/bin/time -v rete export <f>.rete --format nq --sanitize-iris
#         2> <name>.iri.txt | pigz -6  >  <name>.nq.gz
#      then READ THE SUMMARY the sanitizer wrote on stderr (docs/cli.md,
#      "Invalid IRIs"): a `no scheme` count above zero means the dump is not
#      valid N-Quads and no escaping can make it one, so the file is REFUSED
#      (state `failed-schemeless`) and never uploaded — the dataset needs a
#      source rebuild, not another export.
#   3. hf buckets cp  ->  scholar/<dataset>/<name>.nq.gz
#   4. VERIFY by re-listing the object and comparing its byte size
#   5. delete BOTH the .nq.gz AND the .rete if we downloaded it
#   6. only then start the next file
#
#   Step 5 is the one that matters and it is easy to get half-right. The .nq.gz
#   must go too, not just the .rete: at the measured 0.42-0.96x the outputs
#   ALONE would accumulate to roughly 220 GiB against ~197 GiB free, so keeping
#   them "until the end" fails somewhere in the middle with no clear culprit.
#
#   Nothing is ever deleted before step 4 has confirmed the object is in the
#   bucket at the right size. Upload, then verify, then delete — in that order,
#   every time.
#
#   DO NOT PARALLELISE. No concurrent downloads, no upload overlapping the next
#   export. One at a time is slower and it is what makes the disk arithmetic
#   hold; it is also the only arrangement in which a failure can be attributed
#   to a specific file. The `mkdir` lock below enforces one instance.
#
#   LARGEST FIRST. If the disk arithmetic breaks it breaks on crossref (56 GiB),
#   and that should surface in the first hour rather than the twentieth. It also
#   means an interruption leaves the most valuable exports already done.
#
#   The list of files is scripts/scholar-constellation.tsv (dataset, name, URL),
#   derived from dev/SCHOLAR-CONSTELLATION.md. The published Content-Length is
#   read live with HEAD and is the only size that counts: a local file is used
#   only when its size matches the published one exactly, so a stale or
#   differently-built local copy is downloaded rather than silently exported.
#
# THE INVALID-IRI REPORT — what is parsed, and why the numbers are what they are
#
#   `rete export --sanitize-iris` ends with a summary on stderr (stdout is the
#   dump), written by crates/rete-cli/src/commands/iri_report.rs. Three shapes:
#
#     --sanitize-iris: no invalid IRIs found; the dump is byte-identical to a plain export.
#
#     --sanitize-iris: percent-encoded 1 IRI occurrence(s). The dump's IRIs are NOT the
#                      file's IRIs: it no longer joins against the source graph, and
#                      rete → store → rete is no longer the identity.
#                            1  '[' or ']' outside an IP-literal host
#     --sanitize-iris: 1 occurrence(s) CANNOT be repaired by escaping and were written
#                      verbatim — this dump is still not valid N-Quads. Fix them at the
#                      source; a relative IRI needs a base IRI the file never recorded.
#                            1  no scheme — a relative IRI, not an absolute one
#                               e.g. <noscheme/thing>
#
#   Every class row is `<spaces><count>  <reason>`; the five reasons are fixed
#   strings in crates/rete-core/src/iri.rs (IriDefect::reason). The counts are
#   IRI OCCURRENCES (one per term, the graph label once per graph), not
#   statements; the build-time warning is the one that counts statements. A run
#   whose stderr carries no `--sanitize-iris:` line at all is recorded `failed`:
#   an unaudited dump is not something this script will publish.
#
# THINGS LEARNED THE HARD WAY, encoded here rather than in a comment elsewhere
#
#   A shell pipeline masks exit status. `rete export … | pigz > out` returns
#   PIGZ's status, so a failed export yields a valid, TRUNCATED .gz that exits 0.
#   Both `set -o pipefail` and an explicit ${PIPESTATUS[0]} check are used, and
#   the output is then fully decompressed to prove the gzip stream is complete.
#
#   The uploader's exit code is not the verdict. A dry-run that only printed
#   usage once exited 0 and looked like a successful upload. Every file is
#   verified by RE-LISTING the bucket and comparing bytes.
#
#   Disk is the binding constraint, and two concurrent runs each passed their own
#   free-space check and jointly took 68 GB. The `mkdir` lock is the atomic
#   test-and-set that makes one instance the only decision-maker, and the space
#   check is per-file against THAT file's measured need, never an average.
#
#   Memory is the other constraint: an export over a 50-60 GB graph took the
#   Docker VM down (exit 137) before the container was capped. The cap is
#   RETE_EXPORT_MEM; the peak RSS of every export is recorded (`rss_mb`) so the
#   cap for the next, bigger file is a measurement, not a guess.
#
# Options:
#   --all               every row of the manifest, LARGEST first
#   --dry-run           plan only: locate, size, space-check; export nothing
#   --audit-only        export to /dev/null: collect the invalid-IRI report, the
#                       peak RSS and the wall time, record `audited`; write no
#                       .nq.gz, upload nothing, need no bucket CLI
#   --no-upload         export for real and verify the gz, but do NOT upload:
#                       the .nq.gz stays in --work/out (implies --keep) and the
#                       row is recorded `done`. Needs no bucket CLI. Every name
#                       it records is also appended to <work>/not-uploaded.txt,
#                       because a `done` row normally means "confirmed in the
#                       bucket" and here it does not. THE PUBLISHER MUST RUN
#                       WITH --recheck: that re-lists the key for every `done`
#                       row and re-does the ones the bucket does not have.
#                       Separating export from publication is deliberate -- the
#                       sweep is a long unattended data job and an audit role
#                       that cannot upload cannot mis-publish.
#   --memory-budget-mb N
#                       pass `--memory-budget-mb N` to `rete export` (rete >=
#                       #247). N caps the resident decoded dictionary/index;
#                       peak RSS tracks it (256 -> 778 MB, 4096 -> 4282 MB
#                       measured on epfl-infoscience) instead of growing to the
#                       whole decompressed dictionary. 0 = unlimited. Unset
#                       leaves rete's own default (4096). Output is
#                       byte-identical at every budget; a budget below the
#                       dictionary size costs time, not correctness.
#   --manifest FILE     dataset<TAB>name<TAB>url   (default: the file above)
#   --bucket NS/NAME    destination bucket (default $RETE_HF_BUCKET)
#   --prefix P          destination prefix (default scholar)
#   --data DIR          where to look for local .rete copies (default $ROOT/data)
#   --work DIR          scratch: downloads + .nq.gz staging (default $ROOT/dev/scholar-nq);
#                       must lie under the checkout, which is what the container mounts
#   --image NAME        docker image carrying `rete` + pigz (default scholarnq-dev:latest)
#   --rete PATH         the rete binary INSIDE the container (default /repo/dev/target/release/rete)
#   --lock NAME         name the single-instance lock (default "default")
#   --headroom-gb N     free space to leave untouched (default 15)
#   --ratio F           conservative .nq.gz / .rete size factor for the space
#                       check (default 1.0; measured 0.64 on ror, 0.83 on
#                       epfl-infoscience, 0.96 on openaire-2021-datasource --
#                       1.0 keeps the check honest, since the ratio is a
#                       property of how literal-heavy a graph is, not a constant)
#   --keep              do not delete downloads or .nq.gz after a verified upload
#                       (or after a schemeless refusal)
#   --force             re-export and re-upload even when the state file says
#                       done, failed-schemeless or audited
#   --recheck           confirm a `done` state record against the bucket before
#                       trusting it (costs one listing per file)
#
# Env: RETE_HF_BUCKET (default katospiegel/rete-public)
#      RETE_EXPORT_MEM (container memory cap, default 12g)
#
# Host tools: curl (downloads, resumable with -C -), hf (the bucket CLI; not
# needed with --audit-only). Everything else runs in Docker.
#
# RESUME — the state file, not the bucket, is the authority
#   dev/scholar-nq/state.tsv is append-only, one line per attempt:
#     1 status  2 name  3 rete_bytes  4 nqgz_bytes  5 quads  6 url  7 utc
#     8 invalid  9 repaired  10 schemeless  11 bracket  12 forbidden  13 hash
#     14 percent  15 rss_mb  16 secs
#   Columns 8-16 were appended later and are empty on older rows; the first
#   seven never move, so old rows still parse. They are:
#     invalid     invalid IRI occurrences in the dump (repaired + unrepairable)
#     repaired    occurrences percent-encoded by --sanitize-iris
#     schemeless  `no scheme` occurrences: NOT repairable, the dump is invalid
#     bracket     '[' or ']' outside an IP-literal host
#     forbidden   a character the IRIREF grammar excludes
#     hash        more than one '#'
#     percent     '%' not followed by two hex digits
#     rss_mb      peak resident set of `rete export` (/usr/bin/time -v; the
#                 container cgroup's memory.peak when time is missing)
#     secs        wall time of `rete export` (seconds)
#   status is done | failed | skipped-disk | failed-schemeless | audited.
#   The LAST line for a name wins. A `done` line is written only after step 4
#   confirmed the bytes, so a resumed run trusts it and touches no network;
#   `failed` and `skipped-disk` lines are retried. A `failed-schemeless` line is
#   NOT retried while the published size is unchanged -- the same file yields
#   the same verdict; a rebuilt file has a new size and is picked up. Likewise a
#   normal run skips a file whose last line is `audited` with schemeless>0 at the
#   same size, and an --audit-only run skips a file already audited (any of the
#   three statuses with column 10 filled) at the same size. --force overrides
#   all of that. Re-listing the bucket to rebuild this costs ~50 minutes, which
#   is why it is kept. Pass --recheck to verify a `done` record anyway.
#   Nothing is ever deleted from the bucket.
#
#   Next to each attempt, dev/scholar-nq/out/<name>.iri.txt keeps the sanitizer's
#   full stderr and <name>.time the /usr/bin/time -v report; both are small and
#   are never purged, they are the audit evidence.
#
# CAUTION: do not edit this file while a copy of it is running -- bash reads a
# script incrementally. Replace it with a temp file + `mv`, or run a copy.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUCKET="${RETE_HF_BUCKET:-katospiegel/rete-public}"
PREFIX="scholar"
MANIFEST="$ROOT/scripts/scholar-constellation.tsv"
DATA_DIR="${RETE_DATA_DIR:-$ROOT/data}"
WORK="$ROOT/dev/scholar-nq"
IMAGE="scholarnq-dev:latest"
RETE_BIN="/repo/dev/target/release/rete"
LOCK_NAME="default"
HEADROOM_GB=15
RATIO="1.0"
DRY_RUN=0
AUDIT_ONLY=0
NO_UPLOAD=0
MEM_BUDGET_MB=""
ALL=0
KEEP=0
FORCE=0
RECHECK=0
WANT=()

usage() { awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "$0"; exit "${1:-0}"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --all)          ALL=1; shift ;;
    --dry-run)      DRY_RUN=1; shift ;;
    --audit-only)   AUDIT_ONLY=1; shift ;;
    --no-upload)    NO_UPLOAD=1; KEEP=1; shift ;;
    --memory-budget-mb) MEM_BUDGET_MB="${2:?}"; shift 2 ;;
    --manifest)     MANIFEST="${2:?}"; shift 2 ;;
    --bucket)       BUCKET="${2:?}"; shift 2 ;;
    --prefix)       PREFIX="${2:?}"; shift 2 ;;
    --data)         DATA_DIR="${2:?}"; shift 2 ;;
    --work)         WORK="${2:?}"; shift 2 ;;
    --image)        IMAGE="${2:?}"; shift 2 ;;
    --rete)         RETE_BIN="${2:?}"; shift 2 ;;
    --lock)         LOCK_NAME="${2:?}"; shift 2 ;;
    --headroom-gb)  HEADROOM_GB="${2:?}"; shift 2 ;;
    --ratio)        RATIO="${2:?}"; shift 2 ;;
    --keep)         KEEP=1; shift ;;
    --force)        FORCE=1; shift ;;
    --recheck)      RECHECK=1; shift ;;
    -h|--help)      usage 0 ;;
    -*)             echo "unknown option: $1" >&2; usage 2 ;;
    *)              WANT+=("$1"); shift ;;
  esac
done

mkdir -p "$WORK/dl" "$WORK/out"
WORK="$(cd "$WORK" && pwd)"
# The container mounts the checkout at /repo and nothing else writable, so the
# work dir's in-container path is its path relative to the checkout.
case "$WORK" in
  "$ROOT"/*) WORK_IN="/repo/${WORK#"$ROOT"/}" ;;
  *) echo "--work $WORK is not under the checkout $ROOT, the container cannot see it" >&2; exit 2 ;;
esac
LOG="$WORK/export.$LOCK_NAME.log"
STATE="$WORK/state.tsv"
FAILURES="$WORK/failures.$LOCK_NAME.txt"
: > "$FAILURES"
[ -f "$STATE" ] || : > "$STATE"

log()  { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" >> "$LOG"; }
say()  { printf '%s\n' "$*"; log "$*"; }
gb()   { awk -v b="${1:-0}" 'BEGIN{printf "%.2f", b/1073741824}'; }
now()  { date -u +%FT%TZ; }

# ---------------------------------------------------------------------------
# Single-instance lock (see the header). `mkdir` is atomic; a stale lock from a
# killed run is removed by hand -- deliberately, because guessing is how two
# runs end up sharing a disk.
# ---------------------------------------------------------------------------
LOCK="$WORK/.lock-$LOCK_NAME"
if ! mkdir "$LOCK" 2>/dev/null; then
  echo "another export_scholar_nquads.sh holds $LOCK (pid $(cat "$LOCK/pid" 2>/dev/null || echo '?')) -- refusing to start" >&2
  exit 3
fi
echo "$$" > "$LOCK/pid"
cleanup() { rm -f "$LOCK/pid"; rmdir "$LOCK" 2>/dev/null; }
trap cleanup EXIT INT TERM

[ -f "$MANIFEST" ] || { echo "no manifest at $MANIFEST" >&2; exit 2; }

# Prove the bucket CLI answers before trusting an empty listing: an `hf` that is
# missing, unauthenticated or offline lists nothing, which would make every
# object look absent and every file look like it still needs uploading. An
# audit uploads nothing, so it does not need the CLI at all.
if [ "$AUDIT_ONLY" = "0" ] && [ "$NO_UPLOAD" = "0" ] && ! hf buckets ls "$BUCKET" --json >/dev/null 2>&1; then
  echo "cannot list $BUCKET (hf missing, not logged in, or offline)" >&2
  exit 4
fi

# We hold the lock, so $WORK/out is ours alone: anything still in it is debris
# from a killed run, and leaving it there both wastes disk and skews the
# free-space check that the whole schedule depends on. The .iri.txt and .time
# reports are evidence, not debris, and stay.
if [ "$KEEP" = "0" ] && [ "$DRY_RUN" = "0" ]; then
  stale=$(find "$WORK/out" -type f \( -name '*.nq.gz' -o -name '*.exit' \) 2>/dev/null | wc -l)
  [ "$stale" -gt 0 ] && say "purging $stale stale file(s) from $WORK/out"
  find "$WORK/out" -type f \( -name '*.nq.gz' -o -name '*.exit' \) -delete 2>/dev/null
fi

# Host path of $ROOT in the form docker -v wants (D:/... under Git Bash).
host_path() { (cd "$1" && { pwd -W 2>/dev/null || pwd; }); }
ROOT_HOST="$(host_path "$ROOT")"
DATA_HOST="$(host_path "$DATA_DIR")"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
fsize() { stat -c %s "$1" 2>/dev/null || echo 0; }

# Free bytes on the filesystem holding $WORK.
free_bytes() { df -k "$WORK" | awk 'NR==2 {print $4 * 1024}'; }

# Published Content-Length. HEAD every time: the manifest carries URLs, not
# sizes, so there is no stale number to trust.
head_len() {
  curl -sIL --max-time 60 "$1" \
    | grep -i '^content-length:' | tail -1 | tr -d '\r' | awk '{print $2}'
}

# Size of one object in the bucket, or "" when absent. One key, never the whole
# prefix -- a full recursive listing of this bucket takes ~50 minutes.
bucket_size() {
  hf buckets ls "$BUCKET/$1" --recursive --json 2>/dev/null \
    | grep -o '"size": *[0-9]*' | grep -o '[0-9]*' | head -1
}

# The last recorded attempt for a name:
# "status<TAB>nqgz_bytes<TAB>rete_bytes<TAB>schemeless", or "".
state_of() {
  awk -F'\t' -v n="$2" '$2==n {s=$1"\t"$4"\t"$3"\t"$10} END{if(s)print s}' "$1"
}

# Per-attempt measurements, reset for every dataset and appended to the row.
# Empty on paths that never ran the export (HEAD failure, download failure,
# skipped-disk), which is also what every pre-existing row carries.
R_INVALID=""; R_REPAIRED=""; R_SCHEMELESS=""; R_BRACKET=""; R_FORBIDDEN=""
R_HASH=""; R_PERCENT=""; R_RSS_MB=""; R_SECS=""
reset_measures() {
  R_INVALID=""; R_REPAIRED=""; R_SCHEMELESS=""; R_BRACKET=""; R_FORBIDDEN=""
  R_HASH=""; R_PERCENT=""; R_RSS_MB=""; R_SECS=""
}

record() { # status name rete_bytes nqgz_bytes quads url   (+ the R_* measures)
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$1" "$2" "$3" "${4:-}" "${5:-}" "$6" "$(now)" \
    "$R_INVALID" "$R_REPAIRED" "$R_SCHEMELESS" "$R_BRACKET" "$R_FORBIDDEN" \
    "$R_HASH" "$R_PERCENT" "$R_RSS_MB" "$R_SECS" >> "$STATE"
}

# Parse the --sanitize-iris summary (format in the header) into the R_* counts.
# Returns 1 when stderr carries no summary at all. The class rows are matched on
# stable fragments of IriDefect::reason(); the two totals the summary prints
# are cross-checked against the class sum, and a mismatch is logged, because a
# wording change upstream must show up here as a warning and not as a zero.
parse_iri_report() { # file
  local f="$1" parsed
  parsed="$(awk '
    /^--sanitize-iris: no invalid IRIs found/            { seen=1 }
    /^--sanitize-iris: percent-encoded [0-9]+ IRI occ/   { seen=1; rep=$3 }
    /^--sanitize-iris: [0-9]+ occurrence\(s\) CANNOT/    { seen=1; unrep=$2 }
    /^ +[0-9]+  [^ ]/ {
      n=$1; r=$0; sub(/^ +[0-9]+  /, "", r); sub(/ +\[NOT repairable.*$/, "", r)
      if      (r ~ /^no scheme/)                 s+=n
      else if (r ~ /outside an IP-literal host/) b+=n
      else if (r ~ /IRIREF grammar excludes/)    fc+=n
      else if (r ~ /more than one/)              h+=n
      else if (r ~ /not followed by two hex/)    pc+=n
      else                                       unknown+=n
    }
    END {
      if (!seen) exit 1
      printf "%d %d %d %d %d %d %d %d %d\n", rep+unrep, rep, s, b, fc, h, pc, unknown, s+b+fc+h+pc
    }' "$f")" || return 1
  local unknown clsum
  read -r R_INVALID R_REPAIRED R_SCHEMELESS R_BRACKET R_FORBIDDEN R_HASH R_PERCENT unknown clsum <<< "$parsed"
  if [ "$unknown" != "0" ] || [ "$clsum" != "$R_INVALID" ]; then
    say "WARN     $f: class rows sum to $clsum (+$unknown unrecognised) but the summary totals $R_INVALID -- check the report format"
  fi
  return 0
}

# Peak RSS (MB) and wall seconds of the export from the /usr/bin/time -v report,
# with the cgroup peak (from the .exit file) as the fallback for RSS and the
# container's wall clock as the fallback for the time.
parse_time_report() { # time_file exit_file host_secs
  local kb el cg
  kb="$(grep -o 'Maximum resident set size (kbytes): [0-9]*' "$1" 2>/dev/null | grep -o '[0-9]*$')"
  el="$(grep -o 'Elapsed (wall clock) time (h:mm:ss or m:ss): [0-9:.]*' "$1" 2>/dev/null | awk '{print $NF}')"
  cg="$(grep -o 'CG_PEAK=[0-9]*' "$2" 2>/dev/null | grep -o '[0-9]*$')"
  if [ -n "$kb" ]; then R_RSS_MB=$((kb / 1024))
  elif [ -n "$cg" ]; then R_RSS_MB=$((cg / 1048576))
  else R_RSS_MB=""; fi
  if [ -n "$el" ]; then
    R_SECS="$(awk -v t="$el" 'BEGIN{n=split(t,a,":"); s=0; for(i=1;i<=n;i++) s=s*60+a[i]; printf "%.2f", s}')"
  else R_SECS="$3"; fi
  CG_PEAK_MB="${cg:+$((cg / 1048576))}"
}

measures() { # one-line rendering for the log
  printf 'invalid=%s repaired=%s schemeless=%s bracket=%s forbidden=%s hash=%s percent=%s rss_mb=%s secs=%s' \
    "$R_INVALID" "$R_REPAIRED" "$R_SCHEMELESS" "$R_BRACKET" "$R_FORBIDDEN" "$R_HASH" "$R_PERCENT" "$R_RSS_MB" "$R_SECS"
}

# One sweep of $DATA_DIR, up front: size<TAB>path for every local .rete. The
# corpus has ~900k files under it, so re-walking it per dataset would cost more
# than some of the exports.
RETE_INDEX="$WORK/local-rete.$LOCK_NAME.tsv"
find "$DATA_DIR" -name '*.rete' -type f -printf '%s\t%p\n' 2>/dev/null \
  | LC_ALL=C sort -n > "$RETE_INDEX"

# A local .rete whose size EXACTLY matches the published length. Name match
# first (cheap and usually right), then the size index, because several datasets
# keep the published file under a different path (data/openaire/shards/,
# data/*/build/, ...). Size equality is the identity test: these files are
# content-hashed and a differently-built copy is a different length.
find_local() { # name published_len -> path or ""
  local name="$1" want="$2" p
  for p in "$DATA_DIR/$name/$name.rete" "$DATA_DIR/$name/build/$name.rete"; do
    [ -f "$p" ] && [ "$(fsize "$p")" = "$want" ] && { printf '%s' "$p"; return; }
  done
  awk -F'\t' -v w="$want" '$1==w {print $2; exit}' "$RETE_INDEX"
}

# Run a command in the container. /repo is the checkout (rw: the .nq.gz lands
# there), /data is the local .rete corpus, mounted READ-ONLY -- this script must
# never be able to touch the shared checkout it reads from.
# Bounded on purpose: an unbounded export over a 56 GB graph grew until
# Docker Desktop's VM fell over. Override with RETE_EXPORT_MEM.
MEM_LIMIT="${RETE_EXPORT_MEM:-12g}"
# Extra flags for `rete export`. --memory-budget-mb bounds the resident decoded
# dictionary, so the container cap above stops being the thing that decides
# whether a big file exports at all.
EXPORT_FLAGS="--format nq --sanitize-iris"
[ -n "$MEM_BUDGET_MB" ] && EXPORT_FLAGS="$EXPORT_FLAGS --memory-budget-mb $MEM_BUDGET_MB"
in_docker() {
  MSYS_NO_PATHCONV=1 docker run --rm \
    --memory "$MEM_LIMIT" --memory-swap "$MEM_LIMIT" \
    -v "$ROOT_HOST:/repo" -v "$DATA_HOST:/data:ro" \
    -w //repo "$IMAGE" bash -lc "$1"
}

# The in-container prologue/epilogue every export shares: wrap rete in
# /usr/bin/time -v when the image has it (GNU time exits with the child's
# status, so PIPESTATUS still reads rete's), and read the cgroup's peak memory
# on the way out -- cgroup v2 first, v1 second -- as the fallback measurement.
TIME_PROLOGUE='if [ -x /usr/bin/time ]; then T="/usr/bin/time -v -o $TF"; else T=""; echo "no /usr/bin/time in image, rss from cgroup peak" > "$TF"; fi'
CG_EPILOGUE='p=$(cat /sys/fs/cgroup/memory.peak 2>/dev/null || cat /sys/fs/cgroup/memory/memory.max_usage_in_bytes 2>/dev/null || echo); echo "CG_PEAK=$p"'

# ---------------------------------------------------------------------------
# Pick the rows.
# ---------------------------------------------------------------------------
rows=()
while IFS=$'\t' read -r ds name url; do
  [ -n "${url:-}" ] || continue          # blank lines and `# ...` comments
  case "$ds" in \#*) continue ;; esac
  if [ "$ALL" = "1" ]; then rows+=("$ds	$name	$url"); continue; fi
  for w in ${WANT[@]+"${WANT[@]}"}; do
    if [ "$w" = "$name" ] || [ "$w" = "$ds" ]; then rows+=("$ds	$name	$url"); break; fi
  done
done < "$MANIFEST"
if [ ${#rows[@]} -eq 0 ]; then echo "nothing selected: pass names or --all" >&2; usage 2; fi

mode_note=""; [ "$AUDIT_ONLY" = "1" ] && mode_note=", audit-only"
[ "$NO_UPLOAD" = "1" ] && mode_note="$mode_note, no-upload"
[ -n "$MEM_BUDGET_MB" ] && mode_note="$mode_note, memory-budget-mb=$MEM_BUDGET_MB"
say "=== $(now) scholar -> hf://buckets/$BUCKET/$PREFIX/ (dry-run=$DRY_RUN$mode_note, ${#rows[@]} file(s), $(gb "$(free_bytes)") GiB free, $(wc -l < "$RETE_INDEX") local .rete indexed) ==="

# Size every row, then order LARGEST FIRST (see the header).
sized=()
for r in "${rows[@]}"; do
  IFS=$'\t' read -r ds name url <<< "$r"
  len="$(head_len "$url")"
  if [ -z "$len" ]; then
    say "FAIL     $name: no Content-Length from $url"
    echo "$name (HEAD)" >> "$FAILURES"; reset_measures; record failed "$name" "" "" "" "$url"; continue
  fi
  sized+=("$len	$ds	$name	$url")
done
IFS=$'\n' sized=($(printf '%s\n' "${sized[@]}" | LC_ALL=C sort -rn)); unset IFS

ok=0; audited=0; failed=0; skipped=0; schemeless=0; tot_in=0; tot_out=0
for r in "${sized[@]}"; do
  IFS=$'\t' read -r len ds name url <<< "$r"
  key="$PREFIX/$ds/$name.nq.gz"
  out="$WORK/out/$name.nq.gz"
  iri="$WORK/out/$name.iri.txt"
  tfile="$WORK/out/$name.time"
  exitf="$WORK/out/$name.exit"
  reset_measures; CG_PEAK_MB=""

  # -- resume: the state file is the authority --------------------------------
  if [ "$FORCE" = "0" ]; then
    IFS=$'\t' read -r st rec rbytes rsl <<< "$(state_of "$STATE" "$name")"
    if [ "${st:-}" = "done" ] && [ "$AUDIT_ONLY" = "0" ]; then
      if [ "$RECHECK" = "1" ]; then
        landed="$(bucket_size "$key")"
        if [ "${landed:-}" != "${rec:-}" ]; then
          say "REDO     $name (state says $rec, bucket says '${landed:-absent}')"
          st=""
        fi
      fi
      if [ "${st:-}" = "done" ]; then
        say "SKIP     $name (state: done, $rec bytes at $key)"
        skipped=$((skipped+1)); tot_in=$((tot_in+len)); tot_out=$((tot_out+${rec:-0})); continue
      fi
    fi
    # The same file gives the same verdict: a refused or audited-schemeless
    # record at the published size is final until the source is rebuilt.
    if [ "$AUDIT_ONLY" = "0" ] && [ "${rbytes:-}" = "$len" ] && [ -n "${rsl:-}" ] && [ "$rsl" -gt 0 ] \
       && { [ "${st:-}" = "failed-schemeless" ] || [ "${st:-}" = "audited" ]; }; then
      say "SKIP     $name (state: $st, schemeless=$rsl at the published size; needs a source rebuild, --force to retry)"
      echo "$name (state: $st schemeless=$rsl)" >> "$FAILURES"
      schemeless=$((schemeless+1)); failed=$((failed+1)); continue
    fi
    if [ "$AUDIT_ONLY" = "1" ] && [ "${rbytes:-}" = "$len" ] && [ -n "${rsl:-}" ] \
       && { [ "${st:-}" = "audited" ] || [ "${st:-}" = "done" ] || [ "${st:-}" = "failed-schemeless" ]; }; then
      say "SKIP     $name (state: $st, schemeless=$rsl at the published size; --force to re-audit)"
      skipped=$((skipped+1)); continue
    fi
  fi

  # -- locate ---------------------------------------------------------------
  src="$(find_local "$name" "$len")"
  downloaded=0
  if [ -n "$src" ]; then
    inpath="/data/${src#"$DATA_DIR"/}"
    say "LOCAL    $name ($(gb "$len") GiB) <- $src"
  else
    src="$WORK/dl/$name.rete"
    inpath="$WORK_IN/dl/$name.rete"
    downloaded=1
    if [ "$(fsize "$src")" = "$len" ]; then downloaded=2; say "CACHED   $name ($(gb "$len") GiB) <- $src"; fi
  fi

  # -- space ----------------------------------------------------------------
  # The measured need for THIS file, not an average: the .rete only if we have
  # to bring it down, plus the .nq.gz we are about to write (none in an audit),
  # plus headroom.
  out_ratio="$RATIO"; [ "$AUDIT_ONLY" = "1" ] && out_ratio=0
  need=$(awk -v l="$len" -v d="$downloaded" -v r="$out_ratio" -v h="$HEADROOM_GB" \
    'BEGIN{printf "%d", (d==1 ? l : 0) + l*r + h*1073741824}')
  freeb="$(free_bytes)"
  if [ "$freeb" -lt "$need" ]; then
    say "SKIP     $name: needs $(gb "$need") GiB free, have $(gb "$freeb") GiB"
    echo "$name (disk: need $(gb "$need") GiB, free $(gb "$freeb") GiB)" >> "$FAILURES"
    record skipped-disk "$name" "$len" "" "" "$url"
    failed=$((failed+1)); continue
  fi

  if [ "$DRY_RUN" = "1" ]; then
    say "PLAN     $name -> $key: $(gb "$len") GiB .rete, need $(gb "$need") GiB free (download=$downloaded)"
    continue
  fi

  # -- 1. download ----------------------------------------------------------
  #
  # A 60 GiB single GET does not survive. crossref died at 13.18 of 60.22 GiB
  # with `curl: (18) end of response with 47044388127 bytes missing` -- the CDN
  # closed the connection early, which is ordinary at this size and duration.
  # The fix is not more --retry: it is to KEEP the partial file and resume into
  # it with `-C -`, over and over, until the byte count matches. Deleting the
  # partial on failure (which this did at first) throws away the only thing that
  # makes the next attempt cheaper than the last.
  #
  # Give up only when three consecutive attempts move ZERO bytes; anything that
  # is still making progress, however slowly, is still worth resuming.
  if [ "$downloaded" = "1" ]; then
    say "GET      $name ($(gb "$len") GiB) <- $url"
    rc=1; stalls=0; attempt=0
    while [ "$attempt" -lt 20 ]; do
      attempt=$((attempt+1))
      before="$(fsize "$src")"
      if [ "$before" = "$len" ]; then rc=0; break; fi
      # --retry-all-errors so curl itself retries a mid-transfer close (18);
      # --speed-limit/--speed-time abort a transfer that has gone quiet rather
      # than block the whole sequential sweep on one dead socket;
      # --no-progress-meter because curl's meter is a carriage-return per second
      # and would drown the log this script's progress is read from.
      curl -fL --no-progress-meter --retry 5 --retry-delay 5 --retry-all-errors \
           --speed-limit 4096 --speed-time 120 -C - -o "$src" "$url"
      rc=$?
      after="$(fsize "$src")"
      if [ "$after" = "$len" ]; then rc=0; break; fi
      if [ "$after" = "$before" ]; then stalls=$((stalls+1)); else stalls=0; fi
      say "RESUME   $name attempt $attempt: exit=$rc, $(gb "$after")/$(gb "$len") GiB (+$(gb $((after - before))) GiB, stalls=$stalls)"
      if [ "$stalls" -ge 3 ]; then break; fi
    done
    got="$(fsize "$src")"
    if [ "$rc" != "0" ] || [ "$got" != "$len" ]; then
      say "FAIL     $name: download gave up after $attempt attempt(s), exit=$rc, got $got wanted $len"
      echo "$name (download exit=$rc after $attempt attempts)" >> "$FAILURES"
      record failed "$name" "$len" "" "" "$url"
      failed=$((failed+1))
      # The partial IS the resume state, so it is kept when it is far enough
      # along to be worth resuming; a barely-started one is just dead weight
      # against the free-space check every later file has to pass.
      if [ "$got" -lt $((len / 4)) ]; then
        say "         discarding $(gb "$got") GiB partial (<25%)"; rm -f "$src"
      else
        say "         keeping $(gb "$got") GiB partial for a later resume"
      fi
      continue
    fi
    say "GOT      $name: $got bytes in $attempt attempt(s)"
  fi

  discard() { # keep the disk clean on every failure path
    rm -f "$out" "$exitf"
    if [ "$downloaded" != "0" ] && [ "$KEEP" = "0" ]; then rm -f "$src"; fi
  }

  # -- 2. export ------------------------------------------------------------
  # ${PIPESTATUS[0]} is the whole point: without it a failed export is reported
  # as pigz's clean exit and the truncated .gz looks fine.
  #
  # The whole array is copied in ONE command. `e=${PIPESTATUS[0]}` is itself a
  # command, so it overwrites PIPESTATUS before a second line can read [1] --
  # which is exactly how the first run of this script reported an empty pigz
  # status for two files that had in fact exported cleanly.
  #
  # rete's stderr -- the --sanitize-iris summary -- goes to <name>.iri.txt, not
  # the log, so it can be parsed; /usr/bin/time's own report goes to <name>.time.
  rm -f "$out" "$iri" "$tfile" "$exitf"
  t0=$(date +%s)
  if [ "$AUDIT_ONLY" = "1" ]; then
    say "AUDIT    $name -> /dev/null (report: $iri)"
    in_docker "TF='$WORK_IN/out/$name.time'; $TIME_PROLOGUE; \$T '$RETE_BIN' export '$inpath' $EXPORT_FLAGS > /dev/null 2> '$WORK_IN/out/$name.iri.txt'; echo \"RETE_EXIT=\$? PIGZ_EXIT=0\"; $CG_EPILOGUE" \
      > "$exitf" 2>>"$LOG"
  else
    say "EXPORT   $name -> $out"
    in_docker "set -o pipefail; TF='$WORK_IN/out/$name.time'; $TIME_PROLOGUE; \$T '$RETE_BIN' export '$inpath' $EXPORT_FLAGS 2> '$WORK_IN/out/$name.iri.txt' | pigz -p \$(nproc) -6 > '$WORK_IN/out/$name.nq.gz'; st=(\${PIPESTATUS[@]}); echo \"RETE_EXIT=\${st[0]} PIGZ_EXIT=\${st[1]}\"; $CG_EPILOGUE" \
      > "$exitf" 2>>"$LOG"
  fi
  drc=$?
  ex="$(grep -o 'RETE_EXIT=[0-9]*' "$exitf" | grep -o '[0-9]*')"
  px="$(grep -o 'PIGZ_EXIT=[0-9]*' "$exitf" | grep -o '[0-9]*')"
  t1=$(date +%s)
  parse_time_report "$tfile" "$exitf" "$((t1-t0))"
  if [ "$drc" != "0" ] || [ "${ex:-1}" != "0" ] || [ "${px:-1}" != "0" ]; then
    say "FAIL     $name: export docker=$drc rete=$ex pigz=$px (rss_mb=$R_RSS_MB secs=$R_SECS; stderr in $iri)"
    echo "$name (export rete=$ex pigz=$px)" >> "$FAILURES"; record failed "$name" "$len" "" "" "$url"
    failed=$((failed+1)); discard; continue
  fi

  # -- 2b. the invalid-IRI verdict -------------------------------------------
  if ! parse_iri_report "$iri"; then
    say "FAIL     $name: no --sanitize-iris summary on stderr ($iri) -- an unaudited dump is not published"
    echo "$name (no sanitize report)" >> "$FAILURES"; record failed "$name" "$len" "" "" "$url"
    failed=$((failed+1)); discard; continue
  fi
  say "REPORT   $name: $(measures)${CG_PEAK_MB:+ cgroup_peak_mb=$CG_PEAK_MB}"

  if [ "$AUDIT_ONLY" = "1" ]; then
    record audited "$name" "$len" "" "" "$url"
    audited=$((audited+1)); tot_in=$((tot_in+len))
    [ "$R_SCHEMELESS" -gt 0 ] && schemeless=$((schemeless+1))
    rm -f "$exitf"
    if [ "$downloaded" != "0" ] && [ "$KEEP" = "0" ]; then rm -f "$src"; fi
    say "FREE     $(gb "$(free_bytes)") GiB after $name"
    continue
  fi

  if [ "$R_SCHEMELESS" -gt 0 ]; then
    say "REFUSE   $name: schemeless=$R_SCHEMELESS -- a relative IRI has no scheme, escaping cannot repair it and every strict loader rejects the dump; NOT uploading. The dataset needs a source rebuild (docs/cli.md, Invalid IRIs)."
    echo "$name (schemeless=$R_SCHEMELESS, not uploaded)" >> "$FAILURES"
    record failed-schemeless "$name" "$len" "$(fsize "$out")" "" "$url"
    schemeless=$((schemeless+1)); failed=$((failed+1))
    if [ "$KEEP" = "1" ]; then
      say "         keeping $out (--keep)"; rm -f "$exitf"
    else
      discard
    fi
    continue
  fi
  osize="$(fsize "$out")"

  # A full decompress: pigz fails on a truncated stream or a bad CRC, and the
  # line count is the quad count, which is the number a loader will report.
  lines="$(in_docker "pigz -dc '$WORK_IN/out/$name.nq.gz' | wc -l; exit \${PIPESTATUS[0]}" 2>>"$LOG")"
  vrc=$?
  lines="$(printf '%s' "$lines" | tr -dc '0-9')"
  if [ "$vrc" != "0" ] || [ -z "$lines" ] || [ "$lines" -eq 0 ]; then
    say "FAIL     $name: gzip verify rc=$vrc lines=${lines:-0}"
    echo "$name (gzip verify)" >> "$FAILURES"; record failed "$name" "$len" "$osize" "" "$url"
    failed=$((failed+1)); discard; continue
  fi
  say "OK       $name: $(gb "$len") GiB .rete -> $(gb "$osize") GiB .nq.gz ($(awk -v a="$osize" -v b="$len" 'BEGIN{printf "%.3f", a/b}')x), $lines quads, $((t1-t0))s container, rete ${R_SECS}s peak ${R_RSS_MB} MB"

  # -- 3. upload, then 4. RE-LIST -------------------------------------------
  # --no-upload stops here: the dump is exported and its gzip stream proven
  # complete, which is everything the audit role is allowed to do. The row is
  # `done` so a resumed sweep does not re-export it, and the name goes into
  # not-uploaded.txt so the publisher knows this `done` was never in a bucket.
  if [ "$NO_UPLOAD" = "1" ]; then
    say "HOLD     $name: --no-upload, $out kept, NOT published to $key"
    printf '%s\t%s\t%s\t%s\n' "$name" "$key" "$osize" "$(now)" >> "$WORK/not-uploaded.txt"
    record done "$name" "$len" "$osize" "$lines" "$url"
    ok=$((ok+1)); tot_in=$((tot_in+len)); tot_out=$((tot_out+osize))
    rm -f "$exitf"
    say "FREE     $(gb "$(free_bytes)") GiB after $name"
    continue
  fi
  say "PUT      $name -> $key ($(gb "$osize") GiB)"
  hf buckets cp "$out" "hf://buckets/$BUCKET/$key" >>"$LOG" 2>&1; urc=$?
  landed="$(bucket_size "$key")"
  if [ "${landed:-}" != "$osize" ]; then
    say "FAIL     $name: cp exit=$urc, bucket says '${landed:-absent}', wanted $osize"
    echo "$name (upload verify, cp exit=$urc)" >> "$FAILURES"; record failed "$name" "$len" "$osize" "$lines" "$url"
    failed=$((failed+1)); discard; continue
  fi
  say "VERIFY   $name: $landed bytes confirmed at $key (cp exit=$urc)"
  record done "$name" "$len" "$osize" "$lines" "$url"
  ok=$((ok+1)); tot_in=$((tot_in+len)); tot_out=$((tot_out+osize))

  # -- 5. reclaim, only now that the bucket is confirmed --------------------
  if [ "$KEEP" = "0" ]; then
    rm -f "$out" "$exitf"
    # Only ever delete what WE downloaded. A local copy under $DATA_DIR belongs
    # to the checkout and is mounted read-only anyway.
    if [ "$downloaded" != "0" ]; then rm -f "$src"; fi
  fi
  say "FREE     $(gb "$(free_bytes)") GiB after $name"
  # -- 6. next file ---------------------------------------------------------
done

if [ "$DRY_RUN" = "1" ]; then say "DRY RUN complete"; exit 0; fi
if [ "$AUDIT_ONLY" = "1" ]; then
  say "audit done: audited=$audited skipped=$skipped failed=$failed; $schemeless file(s) with schemeless IRIs; $(gb "$tot_in") GiB .rete read"
  [ "$failed" -eq 0 ]; exit $?
fi
say "done: ok=$ok skipped=$skipped failed=$failed (of which schemeless=$schemeless); $(gb "$tot_in") GiB .rete -> $(gb "$tot_out") GiB .nq.gz"
[ "$NO_UPLOAD" = "1" ] && say "NOTE: --no-upload -- nothing was published. $WORK/not-uploaded.txt lists every 'done' row still only on local disk; the publisher must run with --recheck."
[ "$failed" -eq 0 ]
exit $?
