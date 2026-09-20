#!/usr/bin/env bash
# The scholar export driver's whole loop, end to end, with no bucket and no
# network.
#
#     bash tests/scholar/driver_e2e.sh
#
# WHY THIS FILE EXISTS
#
#   `scripts/export_scholar_nquads.sh` decides what gets published. Its unit
#   pieces are tested — `crates/rete-cli/tests/export_report_roundtrip.rs` pins
#   the stderr parser against the real exporter, `tests/scholar/parse_check.sh`
#   pins the independent parse — but the LOOP that strings them together needed
#   a manifest, HTTP-served `.rete` files and a bucket CLI, so nothing drove it.
#   A gate can be individually correct in every function and still not fire.
#
#   Three substitutions make it runnable, and only three:
#
#     - the manifest is a four-row TSV written here
#     - the published files are served by `python3 -m http.server` in a
#       container, so `head_len`'s HEAD and `curl -C -`'s GET are the real ones
#     - `hf` is tests/scholar/hf_stub.sh on $PATH, backed by a directory
#
#   Everything else is the real thing: the real driver, the real `rete export
#   --sanitize-iris`, the real report parser, the real gate, the real
#   independent parse, the real `state.tsv`.
#
# WHAT IT PROVES
#
#   plan (--dry-run)  ->  export  ->  gate  ->  refuse or publish  ->  verify by
#   re-listing  ->  state.tsv  ->  resume skips what is done
#
#   and, specifically:
#
#     - a dump carrying `<https://::1>` ends `failed-invalid` and is NEVER
#       uploaded. That is the September 2026 incident, as a test.
#     - a dump whose defects are all REPAIRABLE is sanitized and published, so
#       the gate is not simply refusing everything.
#     - an upload that exits 0 and stores nothing is caught by the re-listing
#       and recorded `failed`, not `done`.
#
# TIER
#
#   HOST, like tests/interop/oxigraph.sh: it orchestrates containers, so it
#   cannot itself be one. The rete side and the file server run in the dev
#   image; the driver, curl and the stub run here. The driver's own parse check
#   takes the container route from here (no `oxigraph` on a normal host), which
#   is the half tests/scholar/parse_check.sh cannot reach from inside a
#   container — between them both routes are covered.
#
# Environment:
#   RETE_DEV_IMAGE      the dev image to use. Built if unset (slow the first
#                       time); CI passes the tag it already built.
#   RETE_E2E_PORT       host port for the file server (default 8931)
#   RETE_E2E_TARGET_VOLUME
#                       a docker volume to build rete into, instead of the
#                       checkout's `target/`. On a bind-mounted checkout
#                       (Windows, macOS) this is the difference between a
#                       one-minute build and a ten-minute one.
#   RETE_KEEP_E2E=1     do not delete the work directory on exit

# NOT `set -e`: the driver is EXPECTED to exit non-zero on the runs where a
# dataset is refused, and that exit status is itself an assertion.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DRIVER="$ROOT/scripts/export_scholar_nquads.sh"
WORK="$ROOT/dev/scholar-e2e"
PORT="${RETE_E2E_PORT:-8931}"
BUCKET="e2e/rete-test-bucket"
SRV="rete-scholar-e2e-$$"
DATASETS="clean repairable ipv6 noscheme"

# Scoped, never exported: a global MSYS_NO_PATHCONV breaks `git -C` on MSYS.
dk() { MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' docker "$@"; }
# The form `docker -v` wants: D:/... under Git Bash, /... everywhere else.
host_path() { (cd "$1" && { pwd -W 2>/dev/null || pwd; }); }

fails=0
pass() { printf '  ok   %s\n' "$1"; }
fail() {
  printf '  FAIL %s\n' "$1"
  [ $# -gt 1 ] && printf '%s\n' "$2" | sed 's/^/         /' | head -14
  fails=$((fails + 1))
}
assert_eq() { # NAME EXPECTED ACTUAL
  if [ "$2" = "$3" ]; then pass "$1 [$3]"; else fail "$1" "expected [$2], got [$3]"; fi
}
assert_grep() { # NAME PATTERN FILE
  if [ -f "$3" ] && grep -qE "$2" "$3"; then pass "$1"
  else fail "$1" "no /$2/ in $3:
$(tail -25 "$3" 2>/dev/null)"; fi
}
assert_no_grep() { # NAME PATTERN FILE
  if [ ! -f "$3" ] || ! grep -qE "$2" "$3"; then pass "$1"
  else fail "$1" "unwanted /$2/ in $3:
$(grep -nE "$2" "$3" | head -5)"; fi
}

# shellcheck disable=SC2317  # reached through the trap below, never by a call
cleanup() {
  dk rm -f "$SRV" >/dev/null 2>&1
  [ "${RETE_KEEP_E2E:-0}" = "1" ] || rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

# --- preflight ---------------------------------------------------------------
if ! dk info >/dev/null 2>&1; then
  echo "docker is not answering — start it and re-run; this check orchestrates containers." >&2
  exit 2
fi

IMAGE="${RETE_DEV_IMAGE:-}"
if [ -z "$IMAGE" ]; then
  IMAGE="rete-dev-e2e:local"
  echo "building $IMAGE (set RETE_DEV_IMAGE to reuse one CI already built)"
  dk build -q -f "$ROOT/.devcontainer/Dockerfile" -t "$IMAGE" "$ROOT" >/dev/null || exit 2
fi
echo "dev image: $IMAGE"

rm -rf "$WORK"
mkdir -p "$WORK/www" "$WORK/bin" "$WORK/data" "$WORK/bucket" "$WORK/run"
ROOT_HOST="$(host_path "$ROOT")"

# On LINUX the checkout belongs to whoever is running this and the dev image
# runs as uid 1000; when those differ the export container cannot write the dump
# into the work directory, and the driver reports `rete=1 pigz=1` with the real
# reason two layers down. A CI runner is uid 1001, which is how this was found.
# Windows and macOS bind mounts ignore ownership, so they neither need this nor
# would be helped by it.
case "$(uname -s)" in
  Linux)
    RETE_DOCKER_RUN_ARGS="--user $(id -u):$(id -g)"
    export RETE_DOCKER_RUN_ARGS
    ;;
esac
echo "export container user: ${RETE_DOCKER_RUN_ARGS:-the image default (uid 1000)}"

# --- build rete, and the .rete files the "bucket" will serve ------------------
#
# Debug, not release: the fixtures are nine statements each and the binary is
# only ever asked to export them. A release build would cost minutes of CI for
# no signal.
echo "== building rete and the fixture .rete files =="
cargo_args=(-e CARGO_TARGET_DIR=/repo/target)
if [ -n "${RETE_E2E_TARGET_VOLUME:-}" ]; then
  cargo_args=(-v "$RETE_E2E_TARGET_VOLUME:/ctarget" -e CARGO_TARGET_DIR=/ctarget)
fi
# A registry cache if this machine has one; in CI there is none and cargo
# fetches, exactly as tests/interop/rete_side.sh already does.
if dk volume inspect rete-cargo-registry >/dev/null 2>&1; then
  cargo_args+=(-v rete-cargo-registry:/usr/local/cargo/registry)
fi

# The `$CARGO_TARGET_DIR` in the script below is the CONTAINER's, set by
# cargo_args a few lines up; expanding it here would resolve it against this
# shell, where it means nothing.
# shellcheck disable=SC2016
dk run --rm --user root \
  -e CARGO_HOME=/usr/local/cargo -e RUSTUP_HOME=/usr/local/rustup \
  "${cargo_args[@]}" -v "$ROOT_HOST:/repo" -w /repo "$IMAGE" sh -lc '
    set -e
    cargo build -q -p rete-cli
    install -m 0755 "$CARGO_TARGET_DIR/debug/rete" /repo/dev/scholar-e2e/bin/rete
    for f in clean repairable ipv6 noscheme; do
      # Invalid IRIs make `build` WARN and still succeed — that is the whole
      # premise: the defect survives into the dump and the gate is what must
      # catch it. `--strict` would refuse here and prove nothing downstream.
      /repo/dev/scholar-e2e/bin/rete build "/repo/tests/scholar/fixtures/$f.nt" \
        -o "/repo/dev/scholar-e2e/www/$f.rete" 2>/dev/null
    done
  ' || { echo "could not build rete or the fixtures" >&2; exit 2; }

for d in $DATASETS; do
  [ -s "$WORK/www/$d.rete" ] || { echo "fixture $d.rete was not produced" >&2; exit 2; }
done
pass "built rete and 4 fixture .rete files"

# --- the "published bucket": a file server -----------------------------------
# Real HTTP, because `head_len` reads the published Content-Length with a HEAD
# and that number is the driver's identity test for a local copy. Faking it with
# a file:// path would skip the one thing the manifest is for.
dk rm -f "$SRV" >/dev/null 2>&1
dk run -d --name "$SRV" -p "127.0.0.1:$PORT:8000" \
  -v "$(host_path "$WORK/www"):/srv:ro" -w /srv "$IMAGE" \
  python3 -m http.server 8000 >/dev/null || { echo "could not start the file server" >&2; exit 2; }

ready=0
for _ in $(seq 1 60); do
  if curl -sfI "http://127.0.0.1:$PORT/clean.rete" >/dev/null 2>&1; then ready=1; break; fi
  sleep 0.5
done
[ "$ready" = "1" ] || { echo "the file server never answered on 127.0.0.1:$PORT" >&2; dk logs "$SRV" 2>&1 | tail -10; exit 2; }
pass "the file server answers HEAD with a Content-Length"

# --- the manifest -------------------------------------------------------------
MANIFEST="$WORK/manifest.tsv"
{
  printf '# dataset\tname\turl — written by tests/scholar/driver_e2e.sh\n'
  for d in $DATASETS; do
    printf 'e2e\t%s\thttp://127.0.0.1:%s/%s.rete\n' "$d" "$PORT" "$d"
  done
} > "$MANIFEST"

# --- the stub uploader --------------------------------------------------------
install -m 0755 "$ROOT/tests/scholar/hf_stub.sh" "$WORK/bin/hf"
HF_LOG="$WORK/hf-calls.log"
: > "$HF_LOG"

STATE="$WORK/run/state.tsv"
# Last row for NAME, column N. The driver's own resume logic reads it the same
# way: the LAST line for a name wins.
col() { # NAME N
  awk -F'\t' -v n="$1" -v c="$2" '$2==n {v=$c} END{print v}' "$STATE" 2>/dev/null
}
# `grep -c` PRINTS 0 and EXITS 1 on no match, so a `|| echo 0` fallback emits a
# second zero and every comparison against it fails for a reason that looks
# nothing like the cause. The driver's own `lines()` carries the same warning.
count_lines() { # FILE PATTERN
  if [ -f "$1" ]; then grep -c "$2" "$1"; else echo 0; fi
  return 0
}
uploads() { count_lines "$HF_LOG" '^buckets cp '; }

# Run the driver with the stub on PATH and everything pointed at $WORK.
#   --headroom-gb 1  the default reserve is 15 GiB, which a CI runner does not
#                    have; the fixtures are kilobytes, so the check still runs,
#                    it is just scaled to the machine.
drive() { # LOGFILE [args...]
  local logf="$1"; shift
  PATH="$WORK/bin:$PATH" \
  RETE_E2E_BUCKET_DIR="$WORK/bucket" RETE_E2E_HF_LOG="$HF_LOG" \
  RETE_EXPORT_MEM=2g \
    bash "$DRIVER" \
      --manifest "$MANIFEST" --bucket "$BUCKET" --prefix scholar \
      --data "$WORK/data" --work "$WORK/run" \
      --image "$IMAGE" --rete /repo/dev/scholar-e2e/bin/rete \
      --headroom-gb 1 --lock e2e \
      "$@" >"$logf" 2>&1
  DRIVE_CODE=$?
}

# ============================================================================
echo "== 1. plan: --dry-run touches nothing =="
# ============================================================================
drive "$WORK/1-plan.log" --all --dry-run
assert_eq   "the plan succeeds" 0 "$DRIVE_CODE"
assert_grep "…and plans every row" 'PLAN     ipv6 -> scholar/e2e/ipv6.nq.gz' "$WORK/1-plan.log"
assert_eq   "…exporting nothing"   0 "$(find "$WORK/run/out" -name '*.nq.gz' 2>/dev/null | wc -l)"
assert_eq   "…uploading nothing"   0 "$(uploads)"
assert_eq   "…and writing no state row" 0 "$(count_lines "$STATE" .)"

# ============================================================================
echo "== 2. the sweep: export, gate, refuse or publish =="
# ============================================================================
drive "$WORK/2-sweep.log" --all
# Non-zero: two of the four datasets were refused, and a sweep that refused
# something has not succeeded.
if [ "$DRIVE_CODE" -ne 0 ]; then
  pass "the sweep exits non-zero when a dataset is refused [$DRIVE_CODE]"
else
  fail "the sweep exits non-zero when a dataset is refused" "it exited 0"
fi

echo "-- the clean dataset is published --"
assert_eq   "clean: state is done"        "done" "$(col clean 1)"
assert_eq   "clean: the parse check ran and passed" pass "$(col clean 19)"
assert_eq   "clean: no invalid IRIs"      0 "$(col clean 8)"
assert_grep "clean: the bucket was re-listed, not trusted" \
  'VERIFY   clean: [0-9]+ bytes confirmed' "$WORK/2-sweep.log"
if [ -s "$WORK/bucket/$BUCKET/scholar/e2e/clean.nq.gz" ]; then
  pass "clean: the object is in the bucket"
else
  fail "clean: the object is in the bucket" "$(find "$WORK/bucket" -type f)"
fi
assert_eq "clean: the recorded size is the object's size" \
  "$(stat -c %s "$WORK/bucket/$BUCKET/scholar/e2e/clean.nq.gz" 2>/dev/null)" "$(col clean 4)"

echo "-- repairable defects are sanitized and published --"
assert_eq   "repairable: state is done"   "done" "$(col repairable 1)"
assert_eq   "repairable: parse check passed" pass "$(col repairable 19)"
assert_eq   "repairable: nothing unrepairable" 0 "$(col repairable 17)"
if [ "$(col repairable 9)" -gt 0 ] 2>/dev/null; then
  pass "repairable: the sanitizer rewrote IRIs [$(col repairable 9)]"
else
  fail "repairable: the sanitizer rewrote IRIs" "repaired column = '$(col repairable 9)'"
fi
assert_grep "repairable: the referee accepted the repair" \
  'PARSED   repairable' "$WORK/2-sweep.log"

echo "-- THE REGRESSION: <https://::1> is refused and never uploaded --"
assert_eq   "ipv6: state is failed-invalid" failed-invalid "$(col ipv6 1)"
if [ "$(col ipv6 17)" -gt 0 ] 2>/dev/null; then
  pass "ipv6: unrepairable > 0 is what refused it [$(col ipv6 17)]"
else
  fail "ipv6: unrepairable > 0 is what refused it" "unrepairable column = '$(col ipv6 17)'"
fi
# The defect has no class of its own — that is the whole point of the incident.
if [ "$(col ipv6 18)" -gt 0 ] 2>/dev/null; then
  pass "ipv6: …counted as unclassified, the bucket for defects rete cannot name [$(col ipv6 18)]"
else
  fail "ipv6: …counted as unclassified" "unclassified column = '$(col ipv6 18)'"
fi
assert_eq   "ipv6: schemeless is ZERO — the old gate would have opened" 0 "$(col ipv6 10)"
assert_grep "ipv6: the refusal says why and what to do" \
  'REFUSE   ipv6: unrepairable=[1-9]' "$WORK/2-sweep.log"
assert_no_grep "ipv6: the uploader was never called for it" \
  'buckets cp .*ipv6' "$HF_LOG"
if [ -e "$WORK/bucket/$BUCKET/scholar/e2e/ipv6.nq.gz" ]; then
  fail "ipv6: nothing reached the bucket" "the object is there"
else
  pass "ipv6: nothing reached the bucket"
fi

echo "-- the named unrepairable class still blocks too --"
assert_eq   "noscheme: state is failed-invalid" failed-invalid "$(col noscheme 1)"
if [ "$(col noscheme 10)" -gt 0 ] 2>/dev/null; then
  pass "noscheme: counted in its own class [$(col noscheme 10)]"
else
  fail "noscheme: counted in its own class" "schemeless column = '$(col noscheme 10)'"
fi
assert_no_grep "noscheme: never uploaded" 'buckets cp .*noscheme' "$HF_LOG"

echo "-- exactly two objects, and the disk was reclaimed --"
assert_eq "two uploads, not four" 2 "$(uploads)"
assert_eq "two objects in the bucket" 2 "$(find "$WORK/bucket" -type f | wc -l)"
assert_eq "…and no .nq.gz left on disk" 0 "$(find "$WORK/run/out" -name '*.nq.gz' | wc -l)"
assert_eq "…nor any downloaded .rete"   0 "$(find "$WORK/run/dl" -name '*.rete' | wc -l)"
# The reports are evidence, not debris, and must survive the cleanup.
assert_eq "the sanitizer reports are kept" 4 "$(find "$WORK/run/out" -name '*.iri.txt' | wc -l)"
if [ -n "$(col clean 15)" ] && [ -n "$(col clean 16)" ]; then
  pass "peak RSS and wall time were measured [rss_mb=$(col clean 15) secs=$(col clean 16)]"
else
  fail "peak RSS and wall time were measured" "rss_mb='$(col clean 15)' secs='$(col clean 16)'"
fi

# ============================================================================
echo "== 3. resume: the state file is the authority =="
# ============================================================================
before_uploads="$(uploads)"
drive "$WORK/3-resume.log" --all
assert_grep "clean is skipped on its done row"      'SKIP     clean \(state: done' "$WORK/3-resume.log"
assert_grep "repairable is skipped on its done row" 'SKIP     repairable \(state: done' "$WORK/3-resume.log"
# A refused dataset is NOT retried while the published size is unchanged: the
# same file yields the same verdict, and re-exporting 60 GB to learn that again
# is the cost this rule exists to avoid.
assert_grep "ipv6 is not retried, and says why" \
  'SKIP     ipv6 \(state: failed-invalid.*needs a source rebuild' "$WORK/3-resume.log"
assert_no_grep "nothing was re-exported" 'EXPORT   ' "$WORK/3-resume.log"
assert_eq "no second upload" "$before_uploads" "$(uploads)"

# ============================================================================
echo "== 4. --recheck: a done row is confirmed against the bucket =="
# ============================================================================
# The record says the object is there. Take it away. `--recheck` must notice and
# redo the file — without it, a `done` row is trusted forever.
rm -f "$WORK/bucket/$BUCKET/scholar/e2e/clean.nq.gz"
drive "$WORK/4-recheck.log" --all --recheck
assert_grep "the missing object is noticed" \
  "REDO     clean \(state says [0-9]+, bucket says 'absent'\)" "$WORK/4-recheck.log"
assert_grep "…and re-exported and re-uploaded" 'VERIFY   clean' "$WORK/4-recheck.log"
assert_grep "…while the intact one is left alone" \
  'SKIP     repairable \(state: done' "$WORK/4-recheck.log"
if [ -s "$WORK/bucket/$BUCKET/scholar/e2e/clean.nq.gz" ]; then
  pass "the object is back in the bucket"
else
  fail "the object is back in the bucket" "$(find "$WORK/bucket" -type f)"
fi

# ============================================================================
echo "== 5. an upload that exits 0 and stores nothing =="
# ============================================================================
# "The uploader's exit code is not the verdict" is one of the lessons written
# into the driver's header. This is that lesson, executed: the stub reports
# success and writes no object, and the re-listing has to catch it.
rm -rf "$WORK/run2"; mkdir -p "$WORK/run2"
STATE="$WORK/run2/state.tsv"
PATH="$WORK/bin:$PATH" \
RETE_E2E_BUCKET_DIR="$WORK/bucket2" RETE_E2E_HF_LOG="$HF_LOG" \
RETE_E2E_HF_DROP=clean RETE_EXPORT_MEM=2g \
  bash "$DRIVER" --manifest "$MANIFEST" --bucket "$BUCKET" --prefix scholar \
    --data "$WORK/data" --work "$WORK/run2" --image "$IMAGE" \
    --rete /repo/dev/scholar-e2e/bin/rete --headroom-gb 1 --lock e2e2 \
    clean >"$WORK/5-drop.log" 2>&1
assert_eq   "clean is recorded failed, not done" failed "$(col clean 1)"
assert_grep "…because the bucket was asked, not the uploader" \
  "FAIL     clean: cp exit=0, bucket says 'absent'" "$WORK/5-drop.log"

STATE="$WORK/run/state.tsv"

echo
if [ "$fails" -eq 0 ]; then
  echo "driver_e2e: all checks passed"
  exit 0
fi
# A failure is only diagnosable from the driver's own logs. Locally they survive
# in $WORK; on a CI runner the whole checkout goes away, so the ones that carry
# the answer are PRINTED. The first run of this in CI failed with
# `rete=1 pigz=1` and nothing else, and the reason — the container could not
# write into the work directory — was sitting in export.e2e.log.
RETE_KEEP_E2E=1
echo "driver_e2e: $fails check(s) failed"
echo
echo "== state.tsv =="
cat "$WORK/run/state.tsv" 2>/dev/null
echo
echo "== the driver's own log (stderr of every container) =="
tail -40 "$WORK/run/export.e2e.log" 2>/dev/null
echo
echo "== the sanitizer reports =="
for f in "$WORK"/run/out/*.iri.txt; do
  [ -f "$f" ] || continue
  echo "-- $f --"; head -12 "$f"
done
echo
echo "the run is kept in $WORK — 1-plan.log … 5-drop.log, run/state.tsv, run/out/"
exit 1
