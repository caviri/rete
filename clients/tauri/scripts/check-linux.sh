#!/usr/bin/env bash
# Compile, lint and unit-test the desktop client on Linux (see Dockerfile.check).
#
# dist/ and the icons are build outputs; tauri_build needs both to exist. The
# icons here are 1-colour placeholders, enough for a compile check — a release
# generates the real ones with `tauri icon`.
set -euo pipefail
cd "$(dirname "$0")/.."

bash scripts/sync-frontend.sh
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/target/tauri}"

icons=src-tauri/icons
if [ ! -f "$icons/32x32.png" ]; then
  python3 - "$icons" <<'PY'
import os
import struct
import sys
import zlib

d = sys.argv[1]


def png(w, h):
    raw = b"".join(b"\x00" + b"\x00\x00\x00\xff" * w for _ in range(h))

    def chunk(t, b):
        return struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


for name, size in (("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)):
    with open(os.path.join(d, name), "wb") as f:
        f.write(png(size, size))
p = png(32, 32)
with open(os.path.join(d, "icon.ico"), "wb") as f:
    f.write(struct.pack("<HHH", 0, 1, 1) + struct.pack("<BBBBHHII", 32, 32, 0, 0, 1, 32, len(p), 22) + p)
with open(os.path.join(d, "icon.icns"), "wb") as f:
    f.write(b"icns" + struct.pack(">I", 16 + len(p)) + b"ic07" + struct.pack(">I", 8 + len(p)) + p)
PY
fi

cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
