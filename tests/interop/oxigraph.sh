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
#   RETE_OXIGRAPH_IMAGE   default oxigraph/oxigraph:0.5.11 — pinned, because
#                         this test asserts on the referee's exact error
#                         wording, and `:latest` moving is indistinguishable
#                         from rete having broken something
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
OX_IMAGE="${RETE_OXIGRAPH_IMAGE:-oxigraph/oxigraph:0.5.11}"
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
# assert_ne NAME NOT_EXPECTED ACTUAL — for an exit code that must be non-zero,
# where the exact value is the OS's business and only "it refused" is ours.
assert_ne() {
  if [ "$2" != "$3" ]; then pass "$1 [$3]"; else fail "$1" "expected not [$2], got [$3]"; fi
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

# --- quoted triples: the two surfaces, and which one this store reads --------
echo "== Oxigraph: quoted triples =="
# Oxigraph 0.5.x is oxrdf 0.3 / oxttl 0.2 — the RDF 1.2 generation. rete links
# oxrdf 0.2 / oxttl 0.1 and STORES the RDF-star surface, so the exporter writing
# its stored token verbatim produced a dump this store refuses, and a load is
# atomic: zero quads, not "the quoted ones dropped". The exporter now writes the
# ratified triple term by default. The negative case first, because without it
# the positive one only proves the store is lenient.
assert_grep "rete writes RDF 1.2 triple terms by default" '<<\( ' quoted-export.nq
assert_grep "…and the legacy surface on request"          '<<<'   quoted-star.nq
ox load --location /data/store-qstar --file /data/quoted-star.nq >ox_qstar.log 2>&1
assert_grep "Oxigraph REJECTS the RDF-star surface" \
  'must be an IRI, a blank node or a literal' ox_qstar.log
ox dump --location /data/store-qstar --file /data/qstar-back.nq --format nq >/dev/null 2>&1
assert_eq "…and it costs the whole file, as every parse error does" 0 "$(lines qstar-back.nq)"

ox load --location /data/store-quoted --file /data/quoted-export.nq >ox_quoted.log 2>&1
q_code=$?
assert_eq "Oxigraph LOADS the RDF 1.2 dump" 0 "$q_code"
[ $q_code -ne 0 ] && sed 's/^/         > /' ox_quoted.log | head -5
ox dump --location /data/store-quoted --file /data/quoted-back.nq --format nq >/dev/null 2>&1
assert_eq "…and holds every quad the dump had" "$(lines quoted-export.nq)" "$(lines quoted-back.nq)"
# TriG carries the same terms. It is worth its own case because the failure mode
# there is not a rejection: an RDF 1.2 parser reads `<< s p o >>` as a REIFIER,
# so the RDF-star surface would load "successfully" as a different graph.
ox load --location /data/store-qtrig --file /data/quoted-export.trig >ox_qtrig.log 2>&1
assert_eq "the TriG dump loads too" 0 "$?"
ox dump --location /data/store-qtrig --file /data/qtrig-back.nq --format nq >/dev/null 2>&1
assert_eq "…with the same quad count, so nothing was reified into existence" \
  "$(lines quoted-export.nq)" "$(lines qtrig-back.nq)"
# The store's own TriG, for the rete reader to consume further down.
ox dump --location /data/store-qtrig --file /data/qtrig-back.trig --format trig \
  >/dev/null 2>&1

# --- the input surface: rete reading Turtle/TriG back ------------------------
echo "== rete: reading both quoted-triple surfaces =="
# Until `rete build --quoted-triple-syntax`, this whole block was impossible:
# rete's Turtle/TriG reader was oxttl 0.1 and took the RDF-star surface only, so
# rete could not re-ingest the RDF 1.2 TriG it writes BY DEFAULT — an
# interchange format failing its own round trip.
assert_eq "rete re-ingests its own default (RDF 1.2) TriG" \
  0 "$(cat build_trig_12.code 2>/dev/null || echo 99)"
assert_eq "…as the same graph" "$(lines quoted-export.nq)" "$(lines trig12-back.nq)"
assert_eq "rete re-ingests its own RDF-star TriG" \
  0 "$(cat build_trig_star.code 2>/dev/null || echo 99)"
assert_eq "…as the same graph" "$(lines quoted-export.nq)" "$(lines trigstar-back.nq)"
# Both readings of the same data must agree, term for term.
if diff -q <(LC_ALL=C sort trig12-back.nq) <(LC_ALL=C sort trigstar-back.nq) >/dev/null 2>&1; then
  pass "the two input surfaces land one identical graph"
else
  fail "the two input surfaces land one identical graph" \
    "$(diff <(LC_ALL=C sort trig12-back.nq) <(LC_ALL=C sort trigstar-back.nq) | head -8)"
fi

# The refusal, and the hint that makes it actionable. An RDF 1.2 dump read with
# the default input surface CANNOT parse — `<<(` is RDF 1.2's alone — which is
# exactly why that default is safe to keep.
assert_ne "the default input surface refuses an RDF 1.2 dump" \
  0 "$(cat build_trig_wrong.code 2>/dev/null || echo 0)"
assert_grep "…naming the flag that reads it" \
  'quoted-triple-syntax rdf12' build_trig_wrong.err

# The ambiguity, from the other side: the RDF-star TriG read as RDF 1.2 parses
# happily into a DIFFERENT graph — reifiers, blank nodes and `rdf:reifies` that
# the file never wrote. Same bytes, two graphs, and no parser can tell which was
# meant. This is the case the flag exists for, so it is asserted, not described.
assert_eq "the RDF-star TriG also parses as RDF 1.2" \
  0 "$(cat build_trig_reified.code 2>/dev/null || echo 99)"
assert_grep "…as REIFICATION, which is a different graph" \
  '22-rdf-syntax-ns#reifies' reified-back.nq
if [ "$(lines reified-back.nq)" -gt "$(lines quoted-export.nq)" ]; then
  pass "…with more statements than the file has, as reification must produce"
else
  fail "…with more statements than the file has, as reification must produce" \
    "reified=$(lines reified-back.nq) original=$(lines quoted-export.nq)"
fi

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
# The quoted-triple half of the same cycle: what Oxigraph dumps is RDF 1.2, and
# rete's N-Quads tokenizer canonicalises both surfaces to one stored token, so
# the graph must come back unchanged.
assert_eq "the quoted-triple dump rebuilds too" 0 "$(cat build_quoted_back.code 2>/dev/null || echo 99)"
# The TriG half of that cycle, which is the one that needed a new reader: what
# a third-party RDF 1.2 store dumps as TriG, read by rete.
assert_eq "…and so does the store's own RDF 1.2 TriG dump" \
  0 "$(cat build_qtrig_back.code 2>/dev/null || echo 99)"
assert_eq "…as the same graph" "$(lines quoted-export.nq)" "$(lines qtrig-back-export.nq)"
# Blank node LABELS are local to a document — Oxigraph mints its own on load, as
# any conforming store may — so they are masked before the comparison. Everything
# else, including the triple terms and the blank nodes *inside* them, must be
# identical. (The other cycle above uses a fixture with no blank nodes at all,
# which is why it can diff the bytes.)
bmask() { sed -E 's/_:[A-Za-z0-9]+/_:b/g' "$1" | LC_ALL=C sort; }
if diff -q <(bmask quoted-export.nq) <(bmask quoted-back-export.nq) >/dev/null 2>&1; then
  pass "rete → RDF 1.2 → Oxigraph → rete is the identity for quoted triples"
else
  fail "rete → RDF 1.2 → Oxigraph → rete is the identity for quoted triples" \
    "$(diff <(bmask quoted-export.nq) <(bmask quoted-back-export.nq) | head -8)"
fi
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
