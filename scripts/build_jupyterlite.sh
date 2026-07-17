#!/bin/bash
set -euo pipefail
# Kernel 0.8.x ships Pyodide 314 (Python 3.14): its installer understands the
# PEP 783 pyemscripten wheel tags, so `%pip install rete-graph` resolves from
# PyPI directly — requires rete-graph >= 0.2.1 (the 0.2.0 cp314 wheel had a
# wrong-toolchain EH ABI: "cannot resolve symbol invoke_i" at import).
# See docs/clients-dev.md for the per-generation toolchain story.
pip install -q "jupyterlite-core==0.8.*" "jupyterlite-pyodide-kernel==0.8.2" jupyter-server
pip show jupyterlite-core jupyterlite-pyodide-kernel | grep -E "^(Name|Version)"
python - <<'EOF'
import json, pathlib, jupyterlite_pyodide_kernel as k
print("kernel:", k.__version__)
# find the pyodide version constant wherever it lives
root = pathlib.Path(k.__file__).parent
hits = set()
for p in root.rglob("*.py"):
    for line in p.read_text(encoding="utf-8", errors="ignore").splitlines():
        if "PYODIDE_VERSION" in line and "=" in line and '"' in line:
            hits.add(line.strip())
print("\n".join(sorted(hits)[:5]))
EOF

mkdir -p /tmp/contents
cp /io/clients/python/examples/jupyterlite-demo.ipynb /tmp/contents/rete-graph.ipynb
rm -rf /io/docs/jupyterlite
cd /tmp
jupyter lite build --apps lab --no-sourcemaps --contents /tmp/contents --output-dir /io/docs/jupyterlite 2>&1 | tail -5
echo "---- size ----"
du -sh /io/docs/jupyterlite
find /io/docs/jupyterlite -type f | wc -l
