#!/usr/bin/env bash
# A stand-in for the `hf` bucket CLI, installed on $PATH as `hf` by
# tests/scholar/driver_e2e.sh. The bucket is a directory.
#
# It implements exactly the three calls the export driver makes, and no more:
#
#   hf buckets ls  <BUCKET>              --json     the preflight ("does the CLI
#                                                   answer?"); must exit 0
#   hf buckets ls  <BUCKET>/<KEY> --recursive --json  one object's size, which is
#                                                   how the driver VERIFIES an
#                                                   upload — the JSON must carry
#                                                   `"size": N` or the driver
#                                                   reads the object as absent
#   hf buckets cp  <SRC> hf://buckets/<BUCKET>/<KEY>  the upload
#
# WHY A STUB AND NOT A FAKE S3
#
#   The driver's hard-won rule is that the uploader's exit code is not the
#   verdict: a dry-run that only printed usage once exited 0 and looked like a
#   successful upload. Reproducing that needs a `cp` that SUCCEEDS AND STORES
#   NOTHING, which is a behaviour, not an endpoint. RETE_E2E_HF_DROP is that
#   behaviour, and a real bucket could not be asked for it.
#
# Every invocation is appended to $RETE_E2E_HF_LOG, so the harness can assert on
# what was NOT called — "the refused dump was never uploaded" is a claim about
# absence and needs a record to be checked against.
set -u

STORE="${RETE_E2E_BUCKET_DIR:?the harness must point this at a directory}"
LOG="${RETE_E2E_HF_LOG:-/dev/null}"
printf '%s\n' "$*" >> "$LOG"

[ "${1:-}" = "buckets" ] || { echo "stub hf: only 'buckets' is implemented: $*" >&2; exit 2; }

case "${2:-}" in
  ls)
    spec="${3:-}"
    [ -n "$spec" ] || { echo "stub hf: ls needs a bucket" >&2; exit 2; }
    target="$STORE/$spec"
    if [ -f "$target" ]; then
      printf '[{"path": "%s", "size": %s}]\n' "$spec" "$(stat -c %s "$target")"
    else
      # Absent, or a whole-bucket listing: an empty array. The driver's
      # preflight only checks the exit status, and `bucket_size` finds no
      # `"size":` and reads the object as absent — which is what it is.
      printf '[]\n'
    fi
    ;;
  cp)
    src="${3:-}"; dst="${4:-}"
    if [ -z "$src" ] || [ -z "$dst" ]; then
      echo "stub hf: cp needs SRC and DST" >&2; exit 2
    fi
    key="${dst#hf://buckets/}"
    # The failure that taught the driver to re-list: exit 0, store nothing.
    case "${RETE_E2E_HF_DROP:-}" in
      "") ;;
      *) case "$key" in
           *"$RETE_E2E_HF_DROP"*)
             echo "stub hf: DROPPING $key (RETE_E2E_HF_DROP) — exiting 0 with nothing stored" >&2
             exit 0 ;;
         esac ;;
    esac
    target="$STORE/$key"
    mkdir -p "$(dirname "$target")" || exit 1
    cp "$src" "$target" || exit 1
    ;;
  *)
    echo "stub hf: unsupported subcommand: $*" >&2; exit 2 ;;
esac
exit 0
