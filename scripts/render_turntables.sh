#!/bin/sh
# Batch-render turntables for a list of "uuid<TAB>glb_url" lines.
# Run in the rete-blender image WITH the GPU + the OptiX kernel cache mounted:
#   docker run --rm --gpus all -e NVIDIA_DRIVER_CAPABILITIES=all -e BLENDER_GPU=1 \
#     -v "$PWD":/work -v "$PWD/data/.blendercache":/root/.cache -w /work rete-blender \
#     sh scripts/render_turntables.sh data/smithsonian3d/mesh_list.tsv data/smithsonian3d/turntables [limit] [frames] [res]
# Each model -> <out>/<uuid>.webm + <uuid>.gif. Idempotent (skips done ones).
LIST="$1"; OUT="$2"; LIMIT="${3:-0}"; FR="${4:-36}"; RES="${5:-480}"
mkdir -p "$OUT"
TAB=$(printf '\t')
i=0; ok=0
while IFS="$TAB" read -r uid url; do
  [ -z "$uid" ] && continue
  i=$((i + 1))
  [ "$LIMIT" -gt 0 ] && [ "$i" -gt "$LIMIT" ] && break
  if [ -f "$OUT/$uid.webm" ]; then ok=$((ok + 1)); continue; fi
  wget -q -O /tmp/m.glb "$url" 2>/dev/null || { echo "[$i] dl-fail $uid"; continue; }
  sh scripts/glb_to_spin.sh /tmp/m.glb "$OUT/$uid" "$FR" "$RES" 320 >/dev/null 2>&1
  rm -f /tmp/m.glb
  if [ -f "$OUT/$uid.webm" ]; then
    ok=$((ok + 1))
    [ $((i % 10)) -eq 0 ] && echo "[$i] rendered=$ok"
  else
    echo "[$i] render-fail $uid"
  fi
done < "$LIST"
echo "DONE: $ok turntables in $OUT"
