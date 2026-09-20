#!/usr/bin/env bash
# Static analysis for the repository's shell scripts.
#
#     bash tests/shellcheck/run.sh            # the gate; CI runs exactly this
#     bash tests/shellcheck/run.sh --report   # what the uncovered files owe
#
# WHY THIS EXISTS
#
#   `scripts/export_scholar_nquads.sh` is 700+ lines and decides what gets
#   published from the scholar constellation. Until this file, a change to it —
#   or to any of the other 83 tracked shell scripts — ran no check of any kind.
#   The Rust is linted by clippy, the JS by the gate, the shell by nothing.
#
# THE SHAPE OF THE GATE, AND WHY IT IS THIS WAY ROUND
#
#   It checks EVERY tracked `*.sh` except the paths in not-yet-clean.txt. An
#   allow-list would have been easier and would have left every new script
#   unchecked by default, which is the failure mode that produced this gap in
#   the first place. A deny-list makes coverage the default and makes the debt
#   visible in one file.
#
#   The list only shrinks. A path in it that no longer exists is an ERROR, and
#   so is a path in it that has become clean: without that, "fixed it but forgot
#   the list" leaves an entry that silently excuses the next regression.
#
# PINNED, AND IN DOCKER
#
#   A shellcheck version is a set of rules, so a floating one turns an unrelated
#   PR red. The image is pinned; nothing is installed on the host. This is also
#   why there is no "use the local shellcheck if you have one" path — a local
#   binary would be a different rule set reporting different findings.
#
# Environment:
#   RETE_SHELLCHECK_IMAGE   override the pinned image (for trying a new version)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${RETE_SHELLCHECK_IMAGE:-koalaman/shellcheck:v0.10.0}"
EXCLUDE="$ROOT/tests/shellcheck/not-yet-clean.txt"
MODE="${1:-gate}"

# Scoped, never exported: a global MSYS_NO_PATHCONV breaks `git -C` on MSYS,
# and `git -C` is the next line.
dk() { MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' docker "$@"; }
# The form `docker -v` wants: D:/... under Git Bash, /... everywhere else.
ROOT_HOST="$(cd "$ROOT" && { pwd -W 2>/dev/null || pwd; })"

if ! dk info >/dev/null 2>&1; then
  echo "docker is not answering — this check runs shellcheck in a pinned image." >&2
  exit 2
fi

# git, not `find`: dev/ is gitignored scratch and full of shell scripts that
# were never meant to be reviewed, let alone linted.
mapfile -t tracked < <(git -C "$ROOT" ls-files '*.sh' | LC_ALL=C sort)
if [ "${#tracked[@]}" -eq 0 ]; then
  echo "no tracked *.sh found — is $ROOT a checkout?" >&2
  exit 2
fi

# A missing list would silently mean "check everything", which reads as a wall
# of findings rather than as the one thing that is actually wrong.
[ -f "$EXCLUDE" ] || { echo "missing $EXCLUDE — the gate cannot tell debt from a regression without it." >&2; exit 2; }
mapfile -t excluded < <(sed 's/#.*//' "$EXCLUDE" | tr -d '\r' | awk 'NF {print $1}' | LC_ALL=C sort -u)

# Run the pinned checker over FILE...: prints findings, returns 0 when clean.
# (This comment may not open with the tool's own name — shellcheck would read
# `# shellcheck …` as a directive and refuse to parse the file.)
shellcheck_run() {
  dk run --rm -v "$ROOT_HOST:/mnt" -w //mnt "$IMAGE" -f gcc "$@"
}

fails=0

# --- the excluded list must stay honest --------------------------------------
stale=()
now_clean=()
for f in ${excluded[@]+"${excluded[@]}"}; do
  if [ ! -f "$ROOT/$f" ]; then stale+=("$f"); continue; fi
  if shellcheck_run "$f" >/dev/null 2>&1; then now_clean+=("$f"); fi
done

if [ "${#stale[@]}" -gt 0 ]; then
  echo "tests/shellcheck/not-yet-clean.txt names files that do not exist:"
  printf '  %s\n' "${stale[@]}"
  echo "  -> delete those lines."
  fails=$((fails + 1))
fi
if [ "${#now_clean[@]}" -gt 0 ]; then
  echo "tests/shellcheck/not-yet-clean.txt still excuses files that are now CLEAN:"
  printf '  %s\n' "${now_clean[@]}"
  echo "  -> delete those lines, so the next regression in them is caught."
  fails=$((fails + 1))
fi

# --- --report: what the uncovered files owe ----------------------------------
if [ "$MODE" = "--report" ]; then
  echo
  echo "== findings in the files the gate does not cover =="
  for f in ${excluded[@]+"${excluded[@]}"}; do
    [ -f "$ROOT/$f" ] || continue
    out="$(shellcheck_run "$f" 2>&1)"
    [ -n "$out" ] && printf '%s\n' "$out"
  done
  exit 0
fi

# --- the gate ----------------------------------------------------------------
covered=()
for f in "${tracked[@]}"; do
  skip=0
  for x in ${excluded[@]+"${excluded[@]}"}; do
    [ "$f" = "$x" ] && { skip=1; break; }
  done
  [ "$skip" = "0" ] && covered+=("$f")
done

echo "shellcheck $IMAGE"
echo "  ${#covered[@]} file(s) checked, ${#excluded[@]} not yet covered (tests/shellcheck/not-yet-clean.txt)"

# One invocation: shellcheck is fast and a container start is not.
if ! shellcheck_run "${covered[@]}"; then
  echo
  echo "  A finding you are sure is wrong is disabled ON ITS LINE, with the reason:"
  echo "      # shellcheck disable=SC2086  # why this one is deliberate"
  echo "  Never file-wide, and never by adding the file to not-yet-clean.txt —"
  echo "  that list is for scripts nobody has been able to test, not for new ones."
  fails=$((fails + 1))
fi

if [ "$fails" -eq 0 ]; then
  echo "shellcheck: clean"
  exit 0
fi
exit 1
