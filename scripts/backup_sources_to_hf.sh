#!/usr/bin/env bash
# Mirror every dataset's SOURCE data to the durable Hugging Face bucket.
#
# WHY THIS EXISTS
#   A .rete is a DERIVED artifact. The repo has withdrawn its pre-1.0 backwards
#   compatibility promise, so every published graph will be rebuilt at least
#   once. A rebuild needs the source that produced it -- and `getty-tgn` was
#   already lost for good during the 0x02 -> v5 migration because its source
#   was not kept anywhere. scripts/backup_to_hf.sh protects the .rete bytes;
#   this script protects the bytes the .rete was MADE from.
#
#   scripts/backup_sources_to_hf.sh --all --dry-run     # plan the whole corpus
#   scripts/backup_sources_to_hf.sh davidrumsey-maps    # one dataset
#   scripts/backup_sources_to_hf.sh --all               # sweep
#
# KEY CONVENTION (follows the objects already in the bucket -- do not invent a
# second shape):
#   data/<dataset>/raw/<path>  ->  hf  sources/<dataset>/<path>   ("raw/" strips)
#   data/<dataset>/<path>      ->  hf  sources/<dataset>/<path>
# i.e. the key is everything below data/<dataset>/, with a leading raw/ removed,
# because the datasets mirrored by hand (cordis, dblp, ror, zenodo, ...) keep
# their source archives at the top of the dataset directory and have no raw/.
#
# WHAT COUNTS AS SOURCE
#   Everything under raw/ is source by definition and is mirrored unfiltered --
#   a .nt or .ttl in there is a DOWNLOADED ontology, not something we generated.
#   Outside raw/, the derived lanes are filtered out (see FILTER_TOP below):
#   .rete files, parquet/duckdb/sqlite companions, generated N-Triples, spill
#   directories and build logs. All of those are reproducible from the source;
#   the source is not reproducible from them.
#
# Options:
#   --all              every dataset directory under data/ (skips _* and .*)
#   --dry-run          plan only; upload nothing. Prints per-dataset and total
#                      byte counts and exits 0.
#   --bucket NS/NAME   destination bucket (default $RETE_HF_BUCKET)
#   --prefix P         destination prefix (default sources)
#   --data DIR         source tree (default $ROOT/data, override when data/ is
#                      in another checkout)
#   --list             print the dataset -> bucket-prefix mapping and exit
#   --lock NAME        name the single-instance lock (default "default"). Two
#                      sweeps of DISJOINT datasets are safe here in a way they
#                      are not in backup_to_hf.sh -- sources upload straight off
#                      local disk, so there is no staging area and no shared
#                      free-space decision to race on. Give each stream its own
#                      lock name; never point two streams at the same dataset.
#
# Env:
#   RETE_HF_BUCKET     destination bucket (default katospiegel/rete-public)
#   RETE_DATA_DIR      same as --data
#
# Host tools: `hf` (the Hugging Face CLI; the one host exception in this repo).
#
# Resumable and non-destructive: `hf buckets sync --ignore-times` compares by
# BYTE SIZE only, so an object already in the bucket at the identical length is
# skipped and a killed run is restarted by re-running it. --delete is never
# passed: this script only ever adds.
#
# Verification: the uploader's exit code is not the verdict. After syncing, the
# SAME plan is recomputed against a freshly listed bucket; the dataset only
# counts as mirrored when that second plan contains zero remaining uploads,
# which is a size comparison over every single file.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUCKET="${RETE_HF_BUCKET:-katospiegel/rete-public}"
PREFIX="sources"
DATA_DIR="${RETE_DATA_DIR:-$ROOT/data}"
STATE="$ROOT/dev/backup-sources"
LOG="$STATE/backup-sources.log"
DRY_RUN=0
ALL=0
LIST_ONLY=0
LOCK_NAME="default"
DATASETS=()

usage() {
  awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "$0"
  exit "${1:-0}"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --all)      ALL=1; shift ;;
    --dry-run)  DRY_RUN=1; shift ;;
    --bucket)   BUCKET="${2:?--bucket needs ns/name}"; shift 2 ;;
    --prefix)   PREFIX="${2:?--prefix needs a value}"; shift 2 ;;
    --data)     DATA_DIR="${2:?--data needs a directory}"; shift 2 ;;
    --lock)     LOCK_NAME="${2:?--lock needs a name}"; shift 2 ;;
    --list)     LIST_ONLY=1; shift ;;
    -h|--help)  usage 0 ;;
    --) shift; while [ $# -gt 0 ]; do DATASETS+=("$1"); shift; done ;;
    -*) echo "unknown option: $1" >&2; usage 2 ;;
    *)  DATASETS+=("$1"); shift ;;
  esac
done

# ---------------------------------------------------------------------------
# Dataset directory -> bucket prefix.
#
# Almost every dataset maps to its own name. These four do not, and the reason
# is always that the object was mirrored by hand before this script existed --
# the mapping exists so a sweep RECOGNISES those bytes instead of uploading a
# second copy under a different key.
# ---------------------------------------------------------------------------
alias_for() {
  case "$1" in
    davidrumsey-maps) echo "davidrumsey" ;;          # catalog key, and the live
                                                     # manifest mirror already
                                                     # sits at davidrumsey/
    epfl-graph)       echo "graphontology" ;;        # Zenodo record's own name
    openalex)         echo "semopenalex/2025-02-10" ;;  # snapshot-dated by hand
    *)                echo "$1" ;;
  esac
}

# Datasets deliberately NOT swept.
#
#   openaire/orcid/crossref  the source tars ARE already in the bucket under a
#                            hand-made snapshot-dated layout (openaire/2021-v3.0,
#                            orcid/2025, crossref/public-data-file-2026-03) and
#                            were deleted locally afterwards to free disk. What
#                            is left on disk is derived parquet only, so a sweep
#                            would upload nothing but noise under wrong keys.
#   the rest                 scratch, test fixtures and build trees, not datasets.
is_skipped() {
  case "$1" in
    openaire|orcid|crossref) return 0 ;;
    rag|playground|hdt|rdfxml-test|_*|.*) return 0 ;;
    *) return 1 ;;
  esac
}

mkdir -p "$STATE"

# ---------------------------------------------------------------------------
# Single-instance lock. Not paranoia: two concurrent copies of the .rete sweep
# each passed their own free-space check and then jointly took 68 GB. `mkdir`
# is the atomic test-and-set that makes one instance the only decision-maker.
# ---------------------------------------------------------------------------
LOCK="$STATE/.lock-$LOCK_NAME"
if ! mkdir "$LOCK" 2>/dev/null; then
  echo "another backup_sources_to_hf.sh holds $LOCK (pid $(cat "$LOCK/pid" 2>/dev/null || echo '?')) -- refusing to start" >&2
  exit 3
fi
echo "$$" > "$LOCK/pid"
cleanup() { rm -f "$LOCK/pid"; rmdir "$LOCK" 2>/dev/null; }
trap cleanup EXIT INT TERM

log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" >> "$LOG"; }
say() { printf '%s\n' "$*"; log "$*"; }
gb()  { awk -v b="${1:-0}" 'BEGIN{printf "%.2f", b/1073741824}'; }

# ---------------------------------------------------------------------------
# The filters. rsync-style rule files: "- pattern" excludes.
#
# NOTE, hard-won: `hf buckets sync --exclude '*.rete'` silently matches NOTHING
# (only directory-prefix patterns like 'raw/**' work on the command line).
# The same pattern inside --filter-from works. Every file-level rule therefore
# lives in a filter FILE, never in an --exclude flag.
# ---------------------------------------------------------------------------
FILTER_RAW="$STATE/filter-raw.$LOCK_NAME.txt"
FILTER_TOP="$STATE/filter-top.$LOCK_NAME.txt"

# Under raw/, only OS/transfer junk is dropped. Everything else is source.
cat > "$FILTER_RAW" <<'EOF'
- .DS_Store
- Thumbs.db
- *.tmp
- *.partial
- *.crdownload
EOF

# Outside raw/, the derived lanes go. Anything listed here is reproducible from
# the source by a script in this repo; nothing here is irreplaceable.
cat > "$FILTER_TOP" <<'EOF'
- raw/**
- *.rete
- *.parquet
- *.duckdb
- *.sqlite
- *.sqlite3
- *.hdt
- *.pmtiles
- *.nt
- *.nt.gz
- *.nt.bz2
- *.nq
- *.nq.gz
- *.log
- *.err
- *.pid
- *.tmp
- *.partial
- *.profraw
- .DS_Store
- Thumbs.db
- _*
- .git/**
- .cache/**
- .venv/**
- node_modules/**
- __pycache__/**
- spill/**
- _spill/**
- nt/**
- nq/**
- ttl/**
- rdf/**
- triples/**
- parquet/**
- parquet-*/**
- shards/**
- companions/**
- turntables/**
- preview/**
- tables/**
- logs/**
EOF

# ---------------------------------------------------------------------------
# One sync pass. Prints "<uploads>\t<bytes>" on stdout and nothing else, so the
# caller can read it through a command substitution; all chatter goes to stderr.
# ---------------------------------------------------------------------------
sync_pass() {  # src_dir dest_key filter_file mode(plan|apply) -> "n\tbytes"
  local src="$1" dest="$2" filt="$3" mode="$4" out rc n b
  [ -d "$src" ] || { printf '0\t0\n'; return 0; }
  if [ "$mode" = "plan" ]; then
    out=$(hf buckets sync "$src" "hf://buckets/$BUCKET/$dest" \
            --ignore-times --filter-from "$filt" --dry-run 2>>"$LOG")
    rc=$?
  else
    out=$(hf buckets sync "$src" "hf://buckets/$BUCKET/$dest" \
            --ignore-times --filter-from "$filt" --format json 2>>"$LOG")
    rc=$?
    echo "sync exit=$rc for $src -> $dest" >&2
  fi
  # The header line carries the summary; parse it rather than counting rows,
  # which would also count skips.
  n=$(printf '%s' "$out" | head -1 | grep -o '"uploads": *[0-9]*' | grep -o '[0-9]*')
  b=$(printf '%s' "$out" | head -1 | grep -o '"total_size": *[0-9]*' | grep -o '[0-9]*')
  printf '%s\t%s\n' "${n:-0}" "${b:-0}"
}

# ---------------------------------------------------------------------------
# Pick the datasets.
# ---------------------------------------------------------------------------
if [ "$ALL" = "1" ]; then
  while IFS= read -r d; do
    d="${d%/}"; d="${d##*/}"
    is_skipped "$d" && continue
    DATASETS+=("$d")
  done < <(find "$DATA_DIR" -mindepth 1 -maxdepth 1 -type d | LC_ALL=C sort)
fi
if [ ${#DATASETS[@]} -eq 0 ]; then echo "nothing to do: pass dataset names or --all" >&2; usage 2; fi

if [ "$LIST_ONLY" = "1" ]; then
  for ds in "${DATASETS[@]}"; do printf '%s\t%s/%s\n' "$ds" "$PREFIX" "$(alias_for "$ds")"; done
  exit 0
fi

# Prove the CLI answers before trusting an empty listing: an `hf` that is
# missing, unauthenticated or offline lists nothing, which would make every
# object look absent.
if ! hf buckets ls "$BUCKET" --json >/dev/null 2>&1; then
  echo "cannot list $BUCKET (hf missing, not logged in, or offline)" >&2
  exit 4
fi

say "=== $(date -u +%FT%TZ) sources -> hf://buckets/$BUCKET/$PREFIX/ (dry-run=$DRY_RUN, ${#DATASETS[@]} dataset(s)) ==="

tot_n=0; tot_b=0; done_n=0; failed=0
: > "$STATE/failures.$LOCK_NAME.txt"
: > "$STATE/plan.$LOCK_NAME.tsv"

for ds in "${DATASETS[@]}"; do
  src="$DATA_DIR/$ds"
  [ -d "$src" ] || { say "MISSING  $ds (no $src)"; echo "$ds (no local directory)" >> "$STATE/failures.$LOCK_NAME.txt"; failed=$((failed+1)); continue; }
  key="$PREFIX/$(alias_for "$ds")"

  IFS=$'\t' read -r n1 b1 < <(sync_pass "$src/raw" "$key"  "$FILTER_RAW" plan)
  IFS=$'\t' read -r n2 b2 < <(sync_pass "$src"     "$key"  "$FILTER_TOP" plan)
  n=$((n1 + n2)); b=$((b1 + b2))
  printf '%s\t%s\t%s\t%s\n' "$ds" "$key" "$n" "$b" >> "$STATE/plan.$LOCK_NAME.tsv"
  tot_n=$((tot_n + n)); tot_b=$((tot_b + b))

  if [ "$n" -eq 0 ]; then say "OK       $ds -> $key (already complete)"; done_n=$((done_n+1)); continue; fi
  if [ "$DRY_RUN" = "1" ]; then say "PLAN     $ds -> $key: $n file(s), $(gb "$b") GiB"; continue; fi

  say "SYNC     $ds -> $key: $n file(s), $(gb "$b") GiB"
  sync_pass "$src/raw" "$key" "$FILTER_RAW" apply >/dev/null
  sync_pass "$src"     "$key" "$FILTER_TOP" apply >/dev/null

  # Re-plan against a freshly listed bucket. Zero remaining uploads is the only
  # acceptable proof, and it is a byte-size comparison over every file.
  IFS=$'\t' read -r v1 _ < <(sync_pass "$src/raw" "$key" "$FILTER_RAW" plan)
  IFS=$'\t' read -r v2 _ < <(sync_pass "$src"     "$key" "$FILTER_TOP" plan)
  if [ $((v1 + v2)) -eq 0 ]; then
    say "VERIFY   $ds -> $key: 0 remaining, $n file(s) / $(gb "$b") GiB confirmed in the bucket"
    done_n=$((done_n+1))
  else
    say "FAIL     $ds -> $key: $((v1 + v2)) file(s) still missing after sync"
    echo "$ds ($((v1 + v2)) still missing)" >> "$STATE/failures.$LOCK_NAME.txt"
    failed=$((failed+1))
  fi
done

if [ "$DRY_RUN" = "1" ]; then
  say "DRY RUN: $tot_n file(s), $(gb "$tot_b") GiB to upload across ${#DATASETS[@]} dataset(s); $done_n already complete"
  say "per-dataset plan: $STATE/plan.$LOCK_NAME.tsv"
  exit 0
fi
say "datasets verified=$done_n failed=$failed; uploaded $tot_n file(s) / $(gb "$tot_b") GiB"
[ "$failed" -eq 0 ]
exit $?
