#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/rete" <<'FAKE'
#!/bin/sh
set -eu
case "$1" in
  --version) echo "rete 1.0.0-rc.1" ;;
  generate)
    shift
    test "$1" = --output
    mkdir -p "$2"
    for file in rete.bash _rete rete.fish rete.ps1 rete.1; do
      echo "generated $file for rete 1.0.0-rc.1" > "$2/$file"
    done
    ;;
  build)
    while [ "$#" -gt 0 ]; do
      if [ "$1" = -o ]; then shift; : > "$1"; break; fi
      shift
    done
    ;;
  sparql) echo '{"results":{"bindings":[{"name":{"value":"Alice"}}]}}' ;;
  *) echo "unexpected fake rete invocation: $*" >&2; exit 9 ;;
esac
FAKE
chmod +x "$TMP/rete"

cd "$ROOT"
RETE_BINARY="$TMP/rete" DIST_DIR="$TMP/dist" \
  sh scripts/package_release.sh v1.0.0-rc.1 x86_64-unknown-linux-gnu

ARCHIVE="$TMP/dist/rete-1.0.0-rc.1-x86_64-unknown-linux-gnu.tar.gz"
test -s "$ARCHIVE"
tar -tzf "$ARCHIVE" > "$TMP/archive.list"
for file in rete README.md LICENSE CHANGELOG.md rete.bash _rete rete.fish rete.ps1 rete.1; do
  grep -q "/$file$" "$TMP/archive.list"
done

echo "package_release: ok"
