#!/bin/sh
# Render a horizontal-rotation turntable of a .glb as a looping WebM + a GIF.
# Run in the rete-blender image (Blender + ffmpeg + xvfb):
#   sh scripts/glb_to_spin.sh <model.glb> <out_basepath> [frames=36] [res=512] [gifw=360]
# Produces <out_basepath>.webm and <out_basepath>.gif
set -e
GLB="$1"; OUT="$2"; FR="${3:-36}"; RES="${4:-512}"; GIFW="${5:-360}"
[ -n "$GLB" ] && [ -n "$OUT" ] || { echo "usage: glb_to_spin.sh <glb> <outbase> [frames] [res] [gifw]"; exit 2; }
TMP=$(mktemp -d)
xvfb-run -a blender -b -noaudio --python scripts/blender_turntable.py -- "$GLB" "$TMP" "$FR" "$RES" >/dev/null 2>&1 || true
N=$(ls "$TMP"/frame_*.png 2>/dev/null | wc -l)
if [ "$N" -lt 2 ]; then echo "FAIL render: $GLB ($N frames)"; rm -rf "$TMP"; exit 1; fi
# Looping WebM (VP9) — small, plays in the video cell.
ffmpeg -y -framerate 24 -i "$TMP/frame_%03d.png" -c:v libvpx-vp9 -b:v 0 -crf 33 -pix_fmt yuv420p "$OUT.webm" >/dev/null 2>&1
# Palette-optimized GIF (narrower) for the ultra-light image-cell preview.
ffmpeg -y -framerate 18 -i "$TMP/frame_%03d.png" \
  -vf "scale=$GIFW:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer" \
  "$OUT.gif" >/dev/null 2>&1
rm -rf "$TMP"
ls -la "$OUT.webm" "$OUT.gif" 2>/dev/null | awk '{print $5, $9}'
