#!/bin/sh
# Package the add-on as a Blender extension, wheels and all.
#
# The extension bundles the `rete-graph` wheel for every platform, so installing
# it needs no pip, no network and no build tools — Blender unpacks the matching
# wheel into its own site-packages.
#
#   sh clients/blender/build.sh              # download wheels from PyPI, package
#   RETE_WHEEL_DIR=clients/python/dist \
#     sh clients/blender/build.sh            # use locally built wheels instead
#
# Repo convention: everything runs in a container.
#   docker build -t rete-blender clients/blender
#   docker run --rm -v "$PWD":/work -w /work rete-blender sh clients/blender/build.sh
set -eu

VERSION="${RETE_GRAPH_VERSION:-0.3.2}"
HERE="$(cd "$(dirname "$0")" && pwd)"
SOURCE="$HERE/addon"
DIST="$HERE/dist"
# Everything is assembled in a staging copy, so downloading wheels and stamping
# a version never touches the tracked source tree.
STAGE="$DIST/staging"
WHEELS="$STAGE/wheels"

rm -rf "$DIST"
mkdir -p "$DIST" "$WHEELS"
cp "$SOURCE"/*.py "$SOURCE"/blender_manifest.toml "$STAGE/"

# Releases stamp the repository's version onto the extension; a local build
# keeps whatever the manifest already says.
if [ -n "${RETE_ADDON_VERSION:-}" ]; then
    python3 - "$STAGE/blender_manifest.toml" "$RETE_ADDON_VERSION" <<'PY'
import re, sys

path, version = sys.argv[1], sys.argv[2]
text = open(path, encoding="utf-8").read()
text = re.sub(r'^version = "[^"]*"', f'version = "{version}"', text, count=1, flags=re.M)
open(path, "w", encoding="utf-8").write(text)
print(f"version set to {version}")
PY
fi

if [ -n "${RETE_WHEEL_DIR:-}" ]; then
    echo "Using local wheels from $RETE_WHEEL_DIR"
    cp "$RETE_WHEEL_DIR"/rete_graph-*.whl "$WHEELS/"
else
    echo "Downloading rete-graph $VERSION wheels from PyPI"
    # Straight from the release metadata rather than through pip: pip builds
    # tags from --python-version, so asking for 3.11 makes it look for
    # cp311-abi3 and miss the cp39-abi3 wheels that actually work everywhere.
    python3 - "$VERSION" "$WHEELS" <<'PY'
import json, os, sys, urllib.request

version, dest = sys.argv[1], sys.argv[2]
url = f"https://pypi.org/pypi/rete-graph/{version}/json"
with urllib.request.urlopen(url, timeout=60) as response:
    data = json.load(response)

# One wheel per platform; all are abi3, so each covers every Blender Python.
wanted = ("manylinux", "macosx", "win_amd64", "win32")
for entry in data["urls"]:
    name = entry["filename"]
    if not name.endswith(".whl") or not any(tag in name for tag in wanted):
        continue
    target = os.path.join(dest, name)
    with urllib.request.urlopen(entry["url"], timeout=180) as src, open(target, "wb") as out:
        out.write(src.read())
    print(f"  {name}")
PY
fi

echo "Bundled wheels:"
ls -1 "$WHEELS" | sed 's/^/  /'

# Keep the manifest honest: list exactly the wheels that are present.
python3 - "$STAGE/blender_manifest.toml" "$WHEELS" <<'PY'
import os, re, sys

manifest_path, wheel_dir = sys.argv[1], sys.argv[2]
wheels = sorted(f for f in os.listdir(wheel_dir) if f.endswith(".whl"))
if not wheels:
    sys.exit("no wheels were downloaded — cannot package")
block = "wheels = [\n" + "".join(f'  "./wheels/{w}",\n' for w in wheels) + "]"
text = open(manifest_path, encoding="utf-8").read()
text = re.sub(r"wheels = \[[^\]]*\]", block, text, count=1)
open(manifest_path, "w", encoding="utf-8").write(text)
print(f"manifest lists {len(wheels)} wheel(s)")
PY

if command -v blender >/dev/null 2>&1; then
    echo "Validating…"
    blender --command extension validate "$STAGE"
    blender --command extension build --source-dir "$STAGE" --output-dir "$DIST"
else
    echo "Blender not on PATH — zipping without validation"
    (cd "$STAGE" && zip -qr "$DIST/rete_blender.zip" . -x '*__pycache__*' -x '.*')
fi
rm -rf "$STAGE"

echo
ls -lh "$DIST" | sed 's/^/  /'
echo "Install: Blender ▸ Edit ▸ Preferences ▸ Add-ons ▸ ⌄ ▸ Install from Disk…"
