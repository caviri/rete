#!/usr/bin/env bash
# A host that IGNORES `Range` — a real nginx with `max_ranges 0`, which answers
# every range request `200` with the whole file — against the HTTP reader the
# CLI and the native Python wheel share (rete-core's `http` feature).
#
#     bash tests/interop/range_ignoring_host.sh              # the CLI side
#     bash tests/interop/range_ignoring_host.sh --wheel W    # + the wheel W
#     bash tests/interop/range_ignoring_host.sh --wheel W --no-cli
#
# For each side under test:
#
#   1. KEPT     a small `.rete` behind the range-ignoring server answers the
#               same rows as the file read locally, and nginx served it ONCE:
#               one GET, the whole body kept, every later read a slice of it.
#   2. REFUSED  a 300 MiB file behind the same server (sparse, inside the nginx
#               container, so it costs no disk) is refused by name — over the
#               256 MiB cap — and nginx's log shows the body was NOT downloaded.
#   3. CONTROL  the same small file behind an nginx that honours `Range` is read
#               in 206 slices, so case 1 is the fallback and not the default.
#
# Not part of `tests/gate/gate.sh` (it pulls third-party images): CI runs the
# CLI side in the `interop` job and the wheel side in python-test.yml.
#
# Environment:
#   RETE_NGINX_IMAGE    default nginx:1.27-alpine
#   RETE_RUN_IMAGE      the image the CLI binary runs in (default
#                       debian:bookworm-slim, the dev image's base)
#   RETE_PYTHON_IMAGE   the image the wheel runs in (default python:3.11-slim)
#   RETE_DEV_RUN        how to launch the dev container that builds the CLI.
#                       Default `docker compose run --rm -T dev`; CI passes a
#                       `docker run … <pinned image>`.

# NOT `set -e`: some of the assertions are commands that MUST fail.
set -uo pipefail
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

ROOT="$(git rev-parse --show-toplevel)"
WORK="$ROOT/dev/interop-range"
NGINX_IMAGE="${RETE_NGINX_IMAGE:-nginx:1.27-alpine}"
RUN_IMAGE="${RETE_RUN_IMAGE:-debian:bookworm-slim}"
PY_IMAGE="${RETE_PYTHON_IMAGE:-python:3.11-slim}"
DEV_RUN="${RETE_DEV_RUN:-docker compose run --rm -T dev}"

CLI=1
WHEEL=""
while [ $# -gt 0 ]; do
  case "$1" in
    --wheel) WHEEL="${2:?--wheel needs a path}"; shift 2 ;;
    --no-cli) CLI=0; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [ "$CLI" = 0 ] && [ -z "$WHEEL" ]; then
  echo "nothing to test: --no-cli without --wheel" >&2
  exit 2
fi

fails=0
pass() { printf '  ok   %s\n' "$1"; }
fail() {
  printf '  FAIL %s\n' "$1"
  [ $# -gt 1 ] && printf '%s\n' "$2" | sed 's/^/         /' | head -12
  fails=$((fails + 1))
}
assert_eq() {
  if [ "$2" = "$3" ]; then pass "$1 [$3]"; else fail "$1" "expected [$2], got [$3]"; fi
}
assert_grep() {
  if [ -f "$3" ] && grep -qE "$2" "$3"; then
    pass "$1"
  else
    fail "$1" "no /$2/ in $3:
$(head -20 "$3" 2>/dev/null)"
  fi
}

# --- preflight ---------------------------------------------------------------
if ! docker info >/dev/null 2>&1; then
  echo "docker is not answering — start it and re-run." >&2
  exit 2
fi
for image in "$NGINX_IMAGE" "$RUN_IMAGE" ${WHEEL:+"$PY_IMAGE"}; do
  if ! docker pull -q "$image" >/dev/null 2>&1; then
    echo "could not pull $image" >&2
    exit 2
  fi
done

rm -rf "$WORK"
mkdir -p "$WORK"
cp "$ROOT/examples/researchers.nt" "$WORK/graph.nt"
if [ -n "$WHEEL" ]; then
  if [ ! -f "$WHEEL" ]; then
    echo "no wheel at $WHEEL" >&2
    exit 2
  fi
  cp "$WHEEL" "$WORK/"
fi

# max_ranges 0 makes nginx answer a range request as if it had none: 200 and
# the whole file. Port 8081 is the same files with ranges honoured.
cat >"$WORK/nginx.conf" <<'EOF'
events {}
http {
  log_format req '$server_port "$request" $status $body_bytes_sent';
  access_log /dev/stdout req;
  server { listen 8080; root /srv/data; max_ranges 0; location /big/ { root /srv; } }
  server { listen 8081; root /srv/data; }
}
EOF

NET="rete-range-$$"
NGINX="rete-range-nginx-$$"
cleanup() {
  # shellcheck disable=SC2317  # reached through the EXIT trap below
  docker rm -f "$NGINX" >/dev/null 2>&1
  # shellcheck disable=SC2317  # reached through the EXIT trap below
  docker network rm "$NET" >/dev/null 2>&1
}
trap cleanup EXIT
docker network create "$NET" >/dev/null || exit 2
docker run -d --name "$NGINX" --network "$NET" --network-alias nginx --memory 256m \
  -v "$WORK:/srv/data:ro" -v "$WORK/nginx.conf:/etc/nginx/nginx.conf:ro" \
  "$NGINX_IMAGE" >/dev/null || exit 2
# A sparse 300 MiB file in the container's own filesystem: past the cap, and
# no disk spent on it.
docker exec "$NGINX" sh -c 'mkdir -p /srv/big && truncate -s 300M /srv/big/big.rete' || exit 2

# The GETs nginx logged for one path on one port, and the bytes each sent.
gets() { docker logs "$NGINX" 2>/dev/null | grep -c "^$1 \"GET $2 "; return 0; }
sent() { docker logs "$NGINX" 2>/dev/null | grep "^$1 \"GET $2 " | awk '{print $NF}' | tail -1; }
statuses() { docker logs "$NGINX" 2>/dev/null | grep "^$1 \"GET $2 " | awk '{print $(NF-1)}' | sort -u | tr '\n' ' '; }

ROWS_Q='SELECT ?s ?o WHERE { ?s <http://ex/coauthor> ?o } ORDER BY ?s ?o'

# --- the CLI -----------------------------------------------------------------
if [ "$CLI" = 1 ]; then
  echo "== CLI (build in the dev container, run beside nginx) =="
  # shellcheck disable=SC2016  # $CARGO_TARGET_DIR belongs to the dev container
  $DEV_RUN bash -c 'cargo build --release -q -p rete-cli &&
    cp "${CARGO_TARGET_DIR:-target}/release/rete" dev/interop-range/rete &&
    dev/interop-range/rete build dev/interop-range/graph.nt -o dev/interop-range/cli.rete' \
    >"$WORK/cli_build.log" 2>&1
  if [ ! -f "$WORK/rete" ] || [ ! -f "$WORK/cli.rete" ]; then
    fail "the CLI and its fixture were built" "$(tail -20 "$WORK/cli_build.log")"
  else
    # The answer as its IRIs, in order: the URL commands add a traffic line
    # the local one does not print.
    rete() {
      docker run --rm --network "$NET" -v "$WORK:/w" -w /w "$RUN_IMAGE" ./rete "$@" |
        grep -o "<http://ex/[a-z0-9]*>"
    }
    rete sparql /w/cli.rete "$ROWS_Q" >"$WORK/cli_local.txt" 2>"$WORK/cli_local.err"
    rete sparql-url http://nginx:8080/cli.rete "$ROWS_Q" \
      >"$WORK/cli_kept.txt" 2>"$WORK/cli_kept.err"
    assert_eq "CLI: a range-ignoring host answers" 0 "$?"
    if [ -s "$WORK/cli_local.txt" ] && diff -q "$WORK/cli_local.txt" "$WORK/cli_kept.txt" >/dev/null; then
      pass "CLI: …the same rows as the local file ($(grep -c . "$WORK/cli_kept.txt") lines)"
    else
      fail "CLI: …the same rows as the local file" "$(diff "$WORK/cli_local.txt" "$WORK/cli_kept.txt" | head -8)"
    fi
    assert_eq "CLI: …and nginx served the whole file once" 1 "$(gets 8080 /cli.rete)"
    assert_eq "CLI: …as a 200" "200 " "$(statuses 8080 /cli.rete)"

    rete sparql-url http://nginx:8080/big/big.rete 'SELECT * WHERE { ?s ?p ?o } LIMIT 1' \
      >/dev/null 2>"$WORK/cli_big.err"
    code=$?
    if [ "$code" != 0 ]; then pass "CLI: a whole body over the cap is refused (exit $code)"; else fail "CLI: a whole body over the cap is refused" "exit 0"; fi
    assert_grep "CLI: …by name" 'ignored the Range header.*256 MiB is not kept in memory' "$WORK/cli_big.err"
    big_sent="$(sent 8080 /big/big.rete)"
    if [ -n "$big_sent" ] && [ "$big_sent" -lt 314572800 ]; then
      pass "CLI: …without downloading it (nginx sent $big_sent of 314572800 bytes)"
    else
      fail "CLI: …without downloading it" "nginx sent [$big_sent] of 314572800 bytes"
    fi

    rete sparql-url http://nginx:8081/cli.rete "$ROWS_Q" >"$WORK/cli_ranged.txt" 2>"$WORK/cli_ranged.err"
    if diff -q "$WORK/cli_local.txt" "$WORK/cli_ranged.txt" >/dev/null; then
      pass "CLI: control — a range host answers the same rows"
    else
      fail "CLI: control — a range host answers the same rows" "$(head -5 "$WORK/cli_ranged.txt")"
    fi
    assert_eq "CLI: control — …in 206 slices only" "206 " "$(statuses 8081 /cli.rete)"
  fi
fi

# --- the Python wheel --------------------------------------------------------
if [ -n "$WHEEL" ]; then
  echo "== Python wheel ($(basename "$WHEEL")) =="
  cat >"$WORK/py_side.py" <<'EOF'
import sys
import rete_graph as rete

Q = "SELECT ?s ?o WHERE { ?s <http://ex/coauthor> ?o } ORDER BY ?s ?o"
step = sys.argv[1]
if step == "build":
    with open("/w/graph.nt", encoding="utf-8") as f:
        data = rete.build(f.read())
    with open("/w/py.rete", "wb") as f:
        f.write(data)
    print("features:", ",".join(rete.__engine_features__))
elif step in ("kept", "ranged"):
    port = 8080 if step == "kept" else 8081
    local = rete.open("/w/py.rete").query(Q)
    remote = rete.open(f"http://nginx:{port}/py.rete").query(Q)
    rows = lambda rs: [(r["s"].n3, r["o"].n3) for r in rs]
    assert rows(local) == rows(remote), (rows(local), rows(remote))
    print(f"{len(remote)} rows")
elif step == "big":
    try:
        rete.open("http://nginx:8080/big/big.rete")
    except RuntimeError as e:
        print(e)
        sys.exit(3)
    print("opened")
EOF
  py() {
    docker run --rm --network "$NET" -v "$WORK:/w" "$PY_IMAGE" sh -c \
      "pip install -q --no-index --disable-pip-version-check /w/$(basename "$WHEEL") 2>/dev/null &&
       python /w/py_side.py $1"
  }
  py build >"$WORK/py_build.out" 2>&1
  assert_grep "Python: the wheel installs and builds the fixture" '^features:' "$WORK/py_build.out"
  assert_grep "Python: …a native wheel, with the shared reader" 'parallel.*http|http.*parallel' "$WORK/py_build.out"
  py kept >"$WORK/py_kept.out" 2>&1
  assert_eq "Python: a range-ignoring host answers the same rows" 0 "$?"
  [ -s "$WORK/py_kept.out" ] && sed 's/^/         > /' "$WORK/py_kept.out" | tail -3
  assert_eq "Python: …and nginx served the whole file once" 1 "$(gets 8080 /py.rete)"
  py big >"$WORK/py_big.out" 2>&1
  assert_eq "Python: a whole body over the cap is refused (exit 3 = RuntimeError)" 3 "$?"
  assert_grep "Python: …by name" 'ignored the Range header.*256 MiB is not kept in memory' "$WORK/py_big.out"
  py_sent="$(sent 8080 /big/big.rete)"
  if [ -n "$py_sent" ] && [ "$py_sent" -lt 314572800 ]; then
    pass "Python: …without downloading it (nginx sent $py_sent of 314572800 bytes)"
  else
    fail "Python: …without downloading it" "nginx sent [$py_sent] of 314572800 bytes"
  fi
  py ranged >"$WORK/py_ranged.out" 2>&1
  assert_eq "Python: control — a range host answers the same rows" 0 "$?"
  assert_eq "Python: control — …in 206 slices only" "206 " "$(statuses 8081 /py.rete)"
fi

echo
if [ "$fails" -eq 0 ]; then
  echo "range-ignoring host: all checks passed"
  exit 0
fi
echo "range-ignoring host: $fails check(s) failed"
exit 1
