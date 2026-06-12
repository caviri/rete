#!/usr/bin/env bash
# Capture screenshots of all three viewers under xvfb (real WebGL), each bounded
# by `timeout` so a teardown hang can't stall the run. Run in the playwright image.
cd /work/experiments/graph-map
node /work/dev/playwright/serve.mjs /work/experiments/graph-map 8090 & SRV=$!
sleep 2
echo "=== STRUCTURAL ==="
timeout 150 xvfb-run -a node screenshot.mjs http://localhost:8090/viewer.html out/struct 1.2,3.2,5 || echo "STRUCT timeout/err"
echo "=== TOPIC ==="
timeout 150 xvfb-run -a node screenshot.mjs http://localhost:8090/viewer-topics.html out/topic 1.2,4,7 || echo "TOPIC timeout/err"
echo "=== 3D ==="
timeout 150 xvfb-run -a node shot3d.mjs http://localhost:8090/viewer-3d.html out/3d || echo "3D timeout/err"
kill $SRV 2>/dev/null || true
echo "=== DONE ==="
ls -la out/*.png
