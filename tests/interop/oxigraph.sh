#!/usr/bin/env bash
# Prove — against a REAL Oxigraph, in Docker — that `rete export --format nq`
# emits what it names.
#
#     bash tests/interop/oxigraph.sh
#
# Three cases, and the negative one is the point:
#
#   1. NEGATIVE  an unsanitized export of a graph with invalid IRIs must be
#                REJECTED by `oxigraph load`. Without this the other cases would
#                only prove that Oxigraph is lenient.
#   2. POSITIVE  the same graph exported with `--sanitize-iris` must LOAD, and
#                the store must then hold exactly as many quads as the dump had
#                lines.
#   3. HONEST    a graph whose only defect is an IRI with no scheme must STILL
#                be rejected after `--sanitize-iris`, because escaping cannot
#                repair it — and the exporter must have said so on stderr.
#
# Plus the claim on docs/interop.md: the full rete → Oxigraph → rete cycle, on
# clean data (which is what that page was written from) and on repaired data
# (where it is measurably NOT the identity).
#
# NOT part of `tests/gate/gate.sh`. The gate is the browser/playground matrix,
# run after every engine change; this is a CLI interop check that pulls a
# third-party image, so it sits in the same opt-in tier as the other
# network-bound checks — run it by hand, or let CI's `interop` job run it.
#
# Everything runs in containers: the rete side in the repo's dev image, the
# Oxigraph side in `oxigraph/oxigraph`. Nothing is installed on the host.
#
# Environment:
#   RETE_OXIGRAPH_IMAGE   default oxigraph/oxigraph:latest
#   RETE_DEV_RUN          how to launch the dev container. Default
#                         `docker compose run --rm -T dev`; CI passes a
#                         `docker run … <pinned image>` instead so it does not
#                         rebuild the image.

# NOT `set -e`: half the assertions are commands that MUST fail.
set -uo pipefail
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

ROOT="$(git rev-parse --show-toplevel)"
WORK="$ROOT/dev/interop"
FIX="$ROOT/tests/interop/fixtures"
OX_IMAGE="${RETE_OXIGRAPH_IMAGE:-oxigraph/oxigraph:latest}"
DEV_RUN="${RETE_DEV_RUN:-docker compose run --rm -T dev}"

fails=0
pass() { printf '  ok   %s\n' "$1"; }
fail() {
  printf '  FAIL %s\n' "$1"
  [ $# -gt 1 ] && printf '%s\n' "$2" | sed 's/^/         /' | head -12
  fails=$((fails + 1))
}
# assert_eq NAME EXPECTED ACTUAL
assert_eq() {
  if [ "$2" = "$3" ]; then pass "$1 [$3]"; else fail "$1" "expected [$2], got [$3]"; fi
}
# assert_grep NAME PATTERN FILE
assert_grep() {
  if [ -f "$3" ] && grep -qE "$2" "$3"; then
    pass "$1"
  else
    fail "$1" "no /$2/ in $3:
$(head -20 "$3" 2>/dev/null)"
  fi
}
# Non-blank lines in a file, 0 when it is missing or empty. `grep -c` exits 1 on
# zero matches even though it printed the 0, so the status is discarded here
# rather than turned into a second "0" by an `||` fallback.
lines() {
  if [ -f "$1" ]; then grep -c . "$1"; else echo 0; fi
  return 0
}

# --- preflight ---------------------------------------------------------------
if ! docker info >/dev/null 2>&1; then
  echo "docker is not answering — start it and re-run; this check needs two containers." >&2
  exit 2
fi
echo "pulling $OX_IMAGE"
if ! docker pull -q "$OX_IMAGE" >/dev/null 2>&1; then
  echo "could not pull $OX_IMAGE" >&2
  exit 2
fi

rm -rf "$WORK"
mkdir -p "$WORK"
cp "$FIX"/*.nt "$FIX"/*.nq "$WORK/"

ox() { docker run --rm -v "$WORK:/data" "$OX_IMAGE" "$@"; }

# --- the rete side -----------------------------------------------------------
echo "== rete side (dev container) =="
# shellcheck disable=SC2086
$DEV_RUN bash tests/interop/rete_side.sh export
code=$?
echo "  (dev container exit $code)"
if [ $code -ne 0 ]; then
  echo "the rete side did not run; aborting" >&2
  exit $code
fi
cd "$WORK" || exit 2

# --- the build-time audit ----------------------------------------------------
echo "== build: warn, don't refuse =="
assert_eq   "a graph with invalid IRIs still builds" 0 "$(cat build_repairable.code)"
assert_grep "the build warns, with a count" \
  '^warning: 5 statement\(s\) carry an invalid IRI \(5 IRI occurrence\(s\)\)' build_repairable.err
assert_grep "…broken down per class: brackets"  "2  '\[' or '\]' outside an IP-literal host" build_repairable.err
assert_grep "…a second '#'"                     "1  more than one '#'"                       build_repairable.err
assert_grep "…a bad percent-escape"             "1  '%' not followed by two hex digits"      build_repairable.err
assert_grep "…a forbidden character"            '1  a character the IRIREF grammar excludes' build_repairable.err
assert_grep "…and names the flags that act on it" 'sanitize-iris'                            build_repairable.err

echo "== build --strict: refuse =="
if [ "$(cat build_strict.code)" != "0" ]; then
  pass "--strict fails the build (exit $(cat build_strict.code))"
else
  fail "--strict fails the build" "it exited 0"
fi
assert_grep "--strict names the offending IRI" 'invalid IRI <http://example\.org/' build_strict.err
assert_grep "--strict explains the way out"   'hint: .--strict. refuses input'     build_strict.err

echo "== validate: parsing is not validity =="
assert_eq   "validate still succeeds" 0 "$(cat validate_repairable.code)"
assert_grep "validate reports the same audit" 'carry an invalid IRI' validate_repairable.err

echo "== the unrepairable class =="
assert_eq   "no-scheme IRIs still build" 0 "$(cat build_unrepairable.code)"
assert_grep "reported as NOT repairable" 'NOT repairable by escaping' build_unrepairable.err

# --- the exporter ------------------------------------------------------------
echo "== export --sanitize-iris =="
assert_eq   "sanitizing changes no quad count" "$(lines raw.nq)" "$(lines clean.nq)"
assert_grep "the raw dump still carries the bad IRI" 'example\.org/a\[b\]'       raw.nq
assert_grep "the sanitized dump percent-encodes it"  'example\.org/a%5Bb%5D'     clean.nq
assert_grep "…and the second '#'"                    'example\.org/c#d%23e'      clean.nq
assert_grep "…and the stray '%'"                     'example\.org/%25x-'        clean.nq
assert_grep "…and the raw space"                     'example\.org/a%20b'        clean.nq
if grep -q 'raw/caf' clean.nq && ! grep -q 'raw/caf%' clean.nq; then
  pass "a raw ucschar IRI is left alone"
else
  fail "a raw ucschar IRI is left alone" "$(grep 'raw/caf' clean.nq)"
fi
if grep -q 'uchar/caf\\u00E9' clean.nq; then
  pass "a UCHAR escape is left alone"
else
  fail "a UCHAR escape is left alone" "$(grep 'uchar/caf' clean.nq)"
fi
assert_grep "it reports what it rewrote"           'percent-encoded 5 IRI occurrence\(s\)'    export_clean.err
assert_grep "…and that the dump no longer joins"   'no longer joins against the source graph' export_clean.err
assert_grep "the unrepairable export owns up"      '2 occurrence\(s\) CANNOT be repaired'     export_unrep.err
assert_grep "…and says the dump is still invalid"  'still not valid N-Quads'                  export_unrep.err
assert_grep "the no-scheme IRI is written verbatim" '<noscheme/path>'                         unrepairable-sanitized.nq

# --- the real Oxigraph -------------------------------------------------------
echo "== Oxigraph: the NEGATIVE case (this is what makes the rest mean something) =="
# `oxigraph load` prints its parse error and STILL EXITS 0 — a trap for anyone
# scripting a bulk load, and the reason this asserts on the store rather than on
# `$?`. Loads are atomic, so a rejected file leaves an EMPTY store: that is the
# "one bad line costs the whole chunk" mechanism from the issue, measured.
ox load --location /data/store-raw --file /data/raw.nq >ox_raw.log 2>&1
echo "  (oxigraph load exit $? — note: 0 even when it rejected the file)"
assert_grep "Oxigraph REJECTS the unsanitized dump" 'Error while loading file' ox_raw.log
sed 's/^/         > /' ox_raw.log | grep -i 'error' | head -2
ox dump --location /data/store-raw --file /data/raw-back.nq --format nq >/dev/null 2>&1
assert_eq "…and the whole file is lost, not just the bad line" 0 "$(lines raw-back.nq)"

echo "== Oxigraph: the POSITIVE case =="
ox load --location /data/store-clean --file /data/clean.nq >ox_clean.log 2>&1
clean_code=$?
assert_eq "Oxigraph LOADS the sanitized dump" 0 "$clean_code"
[ $clean_code -ne 0 ] && sed 's/^/         > /' ox_clean.log | head -5
ox dump --location /data/store-clean --file /data/clean-back.nq --format nq >ox_dump.log 2>&1
dump_code=$?
assert_eq "Oxigraph dumps the store back out" 0 "$dump_code"
[ $dump_code -ne 0 ] && sed 's/^/         > /' ox_dump.log | head -5
assert_eq "the stored quad count matches the dump" "$(lines clean.nq)" "$(lines clean-back.nq)"

echo "== Oxigraph: the HONEST case (an IRI with no scheme) =="
ox load --location /data/store-unrep --file /data/unrepairable-sanitized.nq >ox_unrep.log 2>&1
assert_grep "a sanitized dump with a relative IRI is STILL rejected" \
  'No scheme found in an absolute IRI' ox_unrep.log
ox dump --location /data/store-unrep --file /data/unrep-back.nq --format nq >/dev/null 2>&1
assert_eq "…and it too costs the whole file" 0 "$(lines unrep-back.nq)"

# --- the cycle docs/interop.md documents -------------------------------------
echo "== rete → Oxigraph → rete =="
ox load --location /data/store-named --file /data/named-export.nq >ox_named.log 2>&1
named_code=$?
assert_eq "the clean named-graph dump loads" 0 "$named_code"
[ $named_code -ne 0 ] && sed 's/^/         > /' ox_named.log | head -5
ox dump --location /data/store-named --file /data/named-back.nq --format nq >/dev/null 2>&1

cd "$ROOT" || exit 2
# shellcheck disable=SC2086
$DEV_RUN bash tests/interop/rete_side.sh rebuild >/dev/null 2>&1
cd "$WORK" || exit 2

assert_eq "the Oxigraph dump rebuilds as a .rete" 0 "$(cat build_back.code 2>/dev/null || echo 99)"
if diff -q <(sort named-export.nq) <(sort named-back-export.nq) >/dev/null 2>&1; then
  pass "on CLEAN data the cycle is the identity, quad for quad"
else
  fail "on CLEAN data the cycle is the identity, quad for quad" \
    "$(diff <(sort named-export.nq) <(sort named-back-export.nq) | head -8)"
fi

# The same cycle on the repaired graph: it comes back with the SAME number of
# quads and DIFFERENT IRIs. That is the cost of `--sanitize-iris`, measured
# rather than asserted — and the reason it is a flag.
assert_eq "the repaired graph also rebuilds" 0 "$(cat build_repaired_back.code 2>/dev/null || echo 99)"
assert_eq "…with the same quad count" "$(lines clean.nq)" "$(lines clean-back-export.nq)"
if diff -q <(sort repairable.nt | grep '^<') <(sort clean-back-export.nq) >/dev/null 2>&1; then
  fail "…but NOT the same IRIs" "the round-trip came back identical, which the sanitizer makes impossible"
else
  pass "…but NOT the same IRIs (the round-trip is lossy, as documented)"
fi

echo
if [ "$fails" -eq 0 ]; then
  echo "interop: all checks passed"
  exit 0
fi
echo "interop: $fails check(s) failed"
exit 1
