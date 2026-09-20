#!/usr/bin/env bash
# The independent parse check of `scripts/export_scholar_nquads.sh`, tested.
#
#     bash tests/scholar/parse_check.sh
#
# WHY THIS FILE EXISTS
#
#   Step 3 of the export driver parses every dump with something that did not
#   produce it, and no dump is recorded `done` without it (PR #256). Until now
#   nothing drove it: the parser lived in `docker run oxigraph/oxigraph`, and a
#   test running inside the dev container has no docker, so the step was
#   verified by hand once and by nothing since. The dev image now carries the
#   pinned `oxigraph` binary itself (.devcontainer/Dockerfile), and the driver
#   prefers a local binary over the container, so the same code path is
#   reachable from inside a container and from a developer's host. This is what
#   that buys.
#
# THE PROPERTY THAT MATTERS MOST IS THE THIRD ONE
#
#   1. a dump that parses is accepted
#   2. a dump that does not parse is REFUSED
#   3. a check that CANNOT RUN is refused too — never quietly passed
#
#   (3) is the one worth a test. "No image", "no docker", "the container died"
#   and "the stream ended early" all mean the dump is UNVERIFIED, and an
#   unverified dump is exactly what this gate exists to stop. It is already
#   implemented and it is one `if` away from silently inverting, so every branch
#   of it is pinned below.
#
# The cannot-run cases are driven with STUBS rather than by really breaking the
# network: deterministic, offline, and they let one run cover branches that no
# single real environment has (a host with docker and a container without it).
#
# NOT part of `tests/gate/gate.sh` — that is the browser matrix. This runs in
# CI's `scholar export driver` job, inside the dev container.
#
# Environment:
#   RETE_SCHOLAR_LOCAL_OPTIONAL=1   do not require a local `oxigraph` (for a
#                                   host that deliberately has none; CI runs
#                                   this inside the dev image, which has one)

# NOT `set -e`: most of the assertions below are commands that MUST fail.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DRIVER="$ROOT/scripts/export_scholar_nquads.sh"
BASH_BIN="$(command -v bash)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

fails=0
pass() { printf '  ok   %s\n' "$1"; }
fail() {
  printf '  FAIL %s\n' "$1"
  [ $# -gt 1 ] && printf '%s\n' "$2" | sed 's/^/         /' | head -12
  fails=$((fails + 1))
}

# The pinned referee version, read from the Dockerfile so the image, the driver
# and this test cannot drift apart: there is one place the number lives.
PIN="$(awk -F= '/^ARG OXIGRAPH_VERSION=/ {print $2}' "$ROOT/.devcontainer/Dockerfile")"

# ---------------------------------------------------------------------------
# Fixtures. Generated here rather than committed: they are three lines each,
# and a dump whose defect is invisible in the diff of the test that asserts on
# it is a dump nobody will maintain.
# ---------------------------------------------------------------------------
mk() { # NAME  (dump on stdin) -> $TMP/NAME.nq.gz
  cat > "$TMP/$1.nq"
  gzip -c "$TMP/$1.nq" > "$TMP/$1.nq.gz"
}

mk clean <<'EOF'
<http://example.org/s> <http://example.org/p> <http://example.org/o> .
<http://example.org/s> <http://example.org/p> "lit"@en <http://example.org/g> .
_:b0 <http://example.org/p> "1"^^<http://www.w3.org/2001/XMLSchema#integer> .
EOF

# What `--sanitize-iris` actually emits, and it must be ACCEPTED: percent-
# encoded brackets and '#', a raw ucschar left alone (RFC 3987 admits it), a
# UCHAR escape left alone, a repaired stray '%'. A referee stricter than the
# repair would refuse every sanitized dump in the corpus, so this case is not
# decoration — it is the one that would catch that.
mk repaired <<'EOF'
<http://example.org/a%5Bb%5D> <http://example.org/p> <http://example.org/c#d%23e> .
<http://example.org/raw/café> <http://example.org/p> <http://example.org/%25x-> .
<http://example.org/uchar/café> <http://example.org/p> <http://example.org/o> .
EOF

# THE REAL BUG (#255/#256). An IPv6 literal in the authority without the
# brackets RFC 3986/3987 requires. rete's classifier had no class for it, read
# zero, and the gate opened; an independent parser does not care what classes
# exist. This line is the regression.
mk ipv6 <<'EOF'
<http://example.org/s> <http://example.org/p> <http://example.org/o> .
<https://::1> <http://example.org/p> <http://example.org/o> .
EOF

# The other unrepairable shape, and the one the exporter DOES name: a relative
# IRI, written verbatim because escaping cannot invent a base.
mk noscheme <<'EOF'
<noscheme/path> <http://example.org/p> <http://example.org/o> .
EOF

# A gzip stream that ends early. `pigz` produces one whenever an export is
# killed, and the decompressed prefix can be perfectly valid N-Quads — which is
# precisely why the gzip exit status is checked separately from the parser's.
head -c 20 "$TMP/clean.nq.gz" > "$TMP/truncated.nq.gz"

# ---------------------------------------------------------------------------
# Stubs for the cannot-run cases.
# ---------------------------------------------------------------------------
stub() { # DIR NAME  (body on stdin)
  mkdir -p "$TMP/$1"
  { echo '#!/bin/sh'; cat; } > "$TMP/$1/$2"
  chmod +x "$TMP/$1/$2"
}

# A docker that cannot produce the image — the message is daemon's, verbatim.
stub bin-noimage docker <<'EOF'
echo "Unable to find image 'oxigraph/oxigraph:0.5.11' locally" >&2
echo "docker: Error response from daemon: manifest for oxigraph/oxigraph:0.5.11 not found: manifest unknown." >&2
exit 125
EOF

# A container that starts and dies without a word. 137 is the OOM kill the
# driver's own header records having met on a 56 GB export.
stub bin-dead docker <<'EOF'
exit 137
EOF

# The same, as a local binary.
stub bin-dead oxigraph-dead <<'EOF'
exit 137
EOF

# A PATH with every directory that provides CMD removed. Real, not simulated:
# this is what "no docker installed" looks like from inside the driver.
path_without() { # CMD -> PATH
  local cmd="$1" out="" d
  local -a dirs
  IFS=: read -ra dirs <<< "$PATH"
  for d in "${dirs[@]}"; do
    [ -n "$d" ] || continue
    [ -x "$d/$cmd" ] && continue
    [ -x "$d/$cmd.exe" ] && continue
    out="${out:+$out:}$d"
  done
  printf '%s' "$out"
}

# ---------------------------------------------------------------------------
# Driving the real driver. CODE/OUT/ERR are globals rather than a return value
# because all three are asserted on, and a pass is only a pass when the exit
# status AND stdout agree.
# ---------------------------------------------------------------------------
CODE=0; OUT=""; ERR=""
drive() { # GZ [VAR=VAL ...]
  local gz="$1"; shift
  env "$@" "$BASH_BIN" "$DRIVER" --parse-check "$gz" \
    >"$TMP/drive.out" 2>"$TMP/drive.err"
  CODE=$?
  OUT="$(cat "$TMP/drive.out")"
  ERR="$(cat "$TMP/drive.err")"
}

expect_pass() { # NAME GZ [VAR=VAL ...]
  local name="$1" gz="$2"; shift 2
  drive "$gz" "$@"
  if [ "$CODE" = "0" ] && [ "${OUT#*parse_check=pass}" != "$OUT" ]; then
    pass "$name"
  else
    fail "$name" "exit $CODE
stdout: $OUT
stderr: $ERR"
  fi
}

# A refusal is three things at once, and the third is the one that rots: a
# non-zero status, an explanation, and NO 'parse_check=pass' anywhere. Asserting
# only the status would let a future refactor print a pass and exit 1.
expect_fail() { # NAME PATTERN GZ [VAR=VAL ...]
  local name="$1" pat="$2" gz="$3"; shift 3
  drive "$gz" "$@"
  if [ "$CODE" = "0" ]; then
    fail "$name" "exited 0 — a dump that cannot be verified must never pass
stdout: $OUT"
    return
  fi
  if [ "${OUT#*parse_check=pass}" != "$OUT" ]; then
    fail "$name" "printed parse_check=pass while failing
stdout: $OUT"
    return
  fi
  if ! printf '%s\n' "$ERR" | grep -qE "$pat"; then
    fail "$name" "no /$pat/ on stderr
stderr: $ERR"
    return
  fi
  pass "$name"
}

echo "== the referee =="
echo "  pinned version: $PIN"
if [ -z "$PIN" ]; then
  fail "the Dockerfile pins an oxigraph version" "no 'ARG OXIGRAPH_VERSION=' line"
else
  pass "the Dockerfile pins an oxigraph version [$PIN]"
fi

# The driver's default image and the image the binary was lifted from must be
# the same release, or the two routes stop being the same code.
if grep -q "RETE_PARSE_IMAGE:-oxigraph/oxigraph:$PIN}" "$DRIVER"; then
  pass "the driver defaults to the same pin"
else
  fail "the driver defaults to the same pin" \
    "$(grep -n 'RETE_PARSE_IMAGE' "$DRIVER" | head -3)"
fi

if command -v oxigraph >/dev/null 2>&1; then
  have_local=1
  got="$(oxigraph --version 2>&1 | head -1)"
  echo "  local binary:   $(command -v oxigraph) — $got"
  case "$got" in
    *"$PIN"*) pass "the local binary is the pinned release" ;;
    *) fail "the local binary is the pinned release" "wanted $PIN, got: $got" ;;
  esac
else
  have_local=0
  if [ "${RETE_SCHOLAR_LOCAL_OPTIONAL:-0}" = "1" ]; then
    echo "  local binary:   none (RETE_SCHOLAR_LOCAL_OPTIONAL=1); the container route is used below"
  else
    fail "the dev image carries a local oxigraph" \
      "not on PATH. This test is meant to run inside the dev container, which
installs it; set RETE_SCHOLAR_LOCAL_OPTIONAL=1 to run on a host without it."
  fi
fi

echo "== a dump that parses =="
expect_pass "a clean dump is accepted"              "$TMP/clean.nq.gz"
# `$OUT` still holds that run. The driver names the route it took, and asserting
# on it is what stops this whole file from quietly testing the container while
# claiming to test the image: inside the dev container there is no docker, so a
# pass here can only have come from the binary in the image.
if [ "$have_local" = "1" ]; then
  if [ "${OUT#*via /}" != "$OUT" ] && [ "${OUT#*docker }" = "$OUT" ]; then
    pass "…by the local binary, not a container [${OUT##*via }]"
  else
    fail "…by the local binary, not a container" "$OUT"
  fi
fi
expect_pass "a --sanitize-iris dump is accepted"    "$TMP/repaired.nq.gz"

echo "== a dump that does not parse =="
# The driver surfaces the referee's own words, because "it failed" is not
# actionable and the column and character are how the offending line is found.
expect_fail "an unbracketed IPv6 authority is REFUSED" \
  "parse_check=fail.*Invalid character" "$TMP/ipv6.nq.gz"
expect_fail "a relative IRI is REFUSED" \
  "parse_check=fail.*No scheme found" "$TMP/noscheme.nq.gz"

echo "== cannot run: every one of these must FAIL CLOSED =="
NO_DOCKER="$(path_without docker)"
GONE=/nonexistent/oxigraph

# 127 is bash's "command not found". Nothing was run, nothing was parsed, and
# the only honest answer is a refusal.
expect_fail "no parser at all (no binary, no docker)" \
  "exited 127.*UNVERIFIED" "$TMP/clean.nq.gz" \
  "RETE_OXIGRAPH_BIN=$GONE" "PATH=$NO_DOCKER"

expect_fail "the image cannot be produced" \
  "Error response from daemon" "$TMP/clean.nq.gz" \
  "RETE_OXIGRAPH_BIN=$GONE" "PATH=$TMP/bin-noimage:$NO_DOCKER"

# The nastiest shape: something ran, said nothing, and exited non-zero. There is
# no error text to grep, so the ONLY thing standing between this and a false
# pass is the explicit exit-status branch.
expect_fail "a container that dies silently" \
  "exited 137.*UNVERIFIED" "$TMP/clean.nq.gz" \
  "RETE_OXIGRAPH_BIN=$GONE" "PATH=$TMP/bin-dead:$NO_DOCKER"

expect_fail "a local binary that dies silently" \
  "exited 137.*UNVERIFIED" "$TMP/clean.nq.gz" \
  "RETE_OXIGRAPH_BIN=$TMP/bin-dead/oxigraph-dead"

# The parser may well accept what it was given here: a truncated .gz decompresses
# to a valid PREFIX. The dump is still short, so the gzip status is checked on
# its own rather than being folded into the parser's.
expect_fail "a gzip stream that ends early" \
  "stream ended early|parse_check=fail" "$TMP/truncated.nq.gz"

echo "== the container route =="
# Both routes must reach the same verdict, which is only checkable where both
# are available — not inside the dev container, which has no docker. Skipped
# loudly rather than silently; when there is no local binary the runs above
# already went through the container, so nothing is lost either way.
if [ "$have_local" = "0" ]; then
  echo "  skip: no local binary, so every check above already used the container"
elif ! command -v docker >/dev/null 2>&1; then
  echo "  skip: no docker here (this is the normal case inside the dev container)"
elif ! docker image inspect "oxigraph/oxigraph:$PIN" >/dev/null 2>&1; then
  echo "  skip: oxigraph/oxigraph:$PIN is not present locally and this test does not pull"
else
  expect_pass "both routes accept the same dump" \
    "$TMP/clean.nq.gz" "RETE_OXIGRAPH_BIN=$GONE"
  expect_fail "both routes refuse the same dump" \
    "parse_check=fail.*Invalid character" "$TMP/ipv6.nq.gz" "RETE_OXIGRAPH_BIN=$GONE"
fi

echo
if [ "$fails" -eq 0 ]; then
  echo "parse_check: all checks passed"
  exit 0
fi
echo "parse_check: $fails check(s) failed"
exit 1
