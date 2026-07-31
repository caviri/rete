#!/usr/bin/env bash
# Build the Quick Look extension and embed it in the app bundle.
#
# No .xcodeproj. An app extension is just a bundle whose binary is linked with
# `-e _NSExtensionMain` instead of a normal main, so swiftc can assemble one
# directly — which keeps this buildable from the same shell step as everything
# else, with no second project file to keep in sync with Cargo.
#
#   ./build.sh <path/to/Rete File Explorer.app> [universal|arm64|x86_64]
#
# macOS only; it is a no-op elsewhere by design so CI can call it unconditionally.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "quicklook: not macOS, skipping" >&2
  exit 0
fi

APP="${1:?usage: build.sh <path to .app> [arch]}"
ARCH="${2:-universal}"

# Resolve the app path against the CALLER's directory before cd-ing to our own.
# Getting this wrong is silent: the copy lands under quicklook/ instead of the
# bundle, every command still succeeds, and the script cheerfully reports having
# embedded the extension into an app that never received it.
if [[ ! -d "$APP" ]]; then
  echo "quicklook: no app bundle at '$APP' (from $(pwd))" >&2
  exit 1
fi
APP="$(cd "$(dirname "$APP")" && pwd)/$(basename "$APP")"

cd "$(dirname "$0")"
NAME="ReteQuickLook"
OUT="build/$NAME.appex"

case "$ARCH" in
  universal) TRIPLES=(arm64-apple-macos11.0 x86_64-apple-macos11.0) ;;
  arm64)     TRIPLES=(arm64-apple-macos11.0) ;;
  x86_64)    TRIPLES=(x86_64-apple-macos11.0) ;;
  *) echo "quicklook: unknown arch '$ARCH'" >&2; exit 1 ;;
esac

rm -rf build
mkdir -p "$OUT/Contents/MacOS"

# swiftc takes one -target at a time, so a universal binary is two compiles
# joined with lipo — the same thing Xcode does behind the scenes.
BINS=()
for triple in "${TRIPLES[@]}"; do
  slice="build/$NAME-${triple%%-*}"
  echo "quicklook: compiling $triple"
  swiftc \
    -target "$triple" \
    -O -swift-version 5 \
    -framework Foundation -framework AppKit -framework Quartz \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$slice" \
    Sources/*.swift
  BINS+=("$slice")
done

if [[ ${#BINS[@]} -gt 1 ]]; then
  lipo -create -output "$OUT/Contents/MacOS/$NAME" "${BINS[@]}"
else
  cp "${BINS[0]}" "$OUT/Contents/MacOS/$NAME"
fi
cp Info.plist "$OUT/Contents/Info.plist"

# An extension must carry a signature to be registered at all. Ad-hoc (`-`) is
# enough for a local test build; a real Developer ID replaces it in CI when the
# secrets exist. Without ANY signature macOS silently ignores the bundle.
# codesign's stderr is kept: a swallowed reason here is the difference between
# "unsigned, as expected" and "the bundle is malformed", which look identical.
codesign --force --sign - --timestamp=none "$OUT" \
  || echo "quicklook: ad-hoc signing the extension failed (it will not register)" >&2

PLUGINS="$APP/Contents/PlugIns"
mkdir -p "$PLUGINS"
rm -rf "$PLUGINS/$NAME.appex"
cp -R "$OUT" "$PLUGINS/"

# Embedding changes the app bundle, so the host's own signature must be redone
# or macOS treats the whole thing as tampered with.
codesign --force --deep --sign - "$APP" \
  || echo "quicklook: re-signing the host app failed (it will not register)" >&2

# Verify rather than announce: every command above can succeed against the wrong
# path, so check the file is actually where the .dmg will pick it up.
EMBEDDED="$PLUGINS/$NAME.appex/Contents/MacOS/$NAME"
if [[ ! -x "$EMBEDDED" ]]; then
  echo "quicklook: embed FAILED — nothing executable at $EMBEDDED" >&2
  exit 1
fi
echo "quicklook: embedded $NAME.appex ($(lipo -archs "$EMBEDDED" 2>/dev/null || echo '?')) into $APP"
