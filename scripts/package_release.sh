#!/bin/sh
# Build and verify one native release archive. Cross-platform CI invokes this
# on a runner matching TARGET, so the generated binary can be smoke-tested.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$ROOT"

TAG=${1:?usage: package_release.sh vVERSION TARGET}
TARGET=${2:?usage: package_release.sh vVERSION TARGET}
case "$TAG" in
  v*) VERSION=${TAG#v} ;;
  *) echo "release tag must start with v: $TAG" >&2; exit 2 ;;
esac

WORKSPACE_VERSION=$(awk \
  '/^\[workspace.package\]/{found=1; next} found && /^version = /{gsub(/[" ]/, "", $3); print $3; exit}' \
  Cargo.toml)
test "$WORKSPACE_VERSION" = "$VERSION" || {
  echo "tag version $VERSION does not match rete-cli $WORKSPACE_VERSION" >&2
  exit 1
}

case "$TARGET" in
  *windows*) EXE=rete.exe; EXT=zip ;;
  *) EXE=rete; EXT=tar.gz ;;
esac

if [ -n "${RETE_BINARY:-}" ]; then
  BINARY=$RETE_BINARY
else
  BINARY="${CARGO_TARGET_DIR:-target}/$TARGET/release/$EXE"
  cargo build --locked --release -p rete-cli --target "$TARGET"
fi
test -x "$BINARY" || { echo "release binary is not executable: $BINARY" >&2; exit 1; }
test "$("$BINARY" --version)" = "rete $VERSION" || {
  echo "release binary version does not match $VERSION" >&2
  exit 1
}

DIST_DIR=${DIST_DIR:-dist}
mkdir -p "$DIST_DIR"
DIST_ABS=$(CDPATH= cd -- "$DIST_DIR" && pwd)
NAME="rete-$VERSION-$TARGET"
STAGE="$DIST_ABS/$NAME"
case "$STAGE" in
  "$DIST_ABS"/rete-*) ;;
  *) echo "refusing unsafe staging path: $STAGE" >&2; exit 1 ;;
esac
rm -rf "$STAGE"
mkdir -p "$STAGE"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
"$BINARY" build crates/rete-core/tests/fixtures/v1/source.nq \
  -o "$TMP/release-smoke.rete" --no-pyramid >/dev/null
"$BINARY" sparql "$TMP/release-smoke.rete" \
  'SELECT ?name WHERE { GRAPH <http://example.test/people> { <http://example.test/alice> <http://example.test/name> ?name } }' \
  --json | grep -q Alice

cp "$BINARY" "$STAGE/$EXE"
cp README.md LICENSE CHANGELOG.md "$STAGE/"
"$BINARY" generate --output "$STAGE"

ARCHIVE="$DIST_ABS/$NAME.$EXT"
rm -f "$ARCHIVE"
if [ "$EXT" = zip ]; then
  command -v 7z >/dev/null || { echo "7z is required to package Windows releases" >&2; exit 1; }
  (cd "$DIST_ABS" && 7z a -tzip "$ARCHIVE" "$NAME" >/dev/null)
else
  tar -C "$DIST_ABS" -czf "$ARCHIVE" "$NAME"
fi

echo "$ARCHIVE"
