#!/usr/bin/env bash
# Build every supported browser artifact from one source revision and record
# the exact toolchain + checksums used for the release candidate.
set -euo pipefail
cd "$(dirname "$0")/.."

ASYNCIFY_TOOLCHAIN="${ASYNCIFY_TOOLCHAIN:-nightly-2026-07-01}"
export ASYNCIFY_TOOLCHAIN

command -v wasm-pack >/dev/null || {
  echo "wasm-pack is required (use the devcontainer image)" >&2
  exit 1
}
command -v wasm-opt >/dev/null || {
  echo "wasm-opt is required (use the devcontainer image)" >&2
  exit 1
}

if [[ -z "${RETE_SOURCE_REVISION:-}" ]]; then
  if [[ -n "${GITHUB_SHA:-}" ]]; then
    RETE_SOURCE_REVISION="$GITHUB_SHA"
  elif RETE_SOURCE_REVISION="$(git rev-parse HEAD 2>/dev/null)"; then
    :
  else
    echo "cannot resolve the source revision inside this worktree mount" >&2
    echo "pass: docker compose run -e RETE_SOURCE_REVISION=<sha> --rm wasm" >&2
    exit 1
  fi
fi
export RETE_SOURCE_REVISION
export RETE_BUILD_STAMP="${RETE_BUILD_STAMP:-$RETE_SOURCE_REVISION}"

# Binaryen v108 is intentionally retained for the reference-types-disabled
# Asyncify pass. It corrupts modern wasm-bindgen externref tables, so regular
# builds explicitly skip wasm-opt. Rust's release profile still optimizes them.
wasm-pack build crates/rete-wasm --target web --out-dir ../../web/pkg --no-opt
wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules --no-opt
# docs/engine/ is the tracked ESM copy the standalone docs pages import
# (anatomy/bim-pair/building). It used to be a hand-copy with no producer —
# refresh it here so the CI parity diff below can actually guard it.
mkdir -p docs/engine
cp web/pkg/rete_wasm.js web/pkg/rete_wasm_bg.wasm docs/engine/
bash scripts/build_playground_async.sh
uv run python scripts/stage_playground_datasets.py
uv run python scripts/build_playground.py
mkdir -p tests/gate/.cache
cargo run -q --release -p rete-cli -- build tests/gate/fixtures/worldcup2026.nt -o tests/gate/.cache/worldcup2026.rete

python3 -P - <<'PY'
import hashlib
import json
import os
from pathlib import Path
import subprocess

root = Path.cwd()
paths = [
    Path("web/pkg/rete_wasm_bg.wasm"),
    Path("web/pkg-nomodules/rete_wasm_bg.wasm"),
    Path("web/pkg-nomodules-async/rete_wasm_bg.wasm"),
    Path("docs/rete_wasm_async.wasm"),
]

def version(*command: str) -> str:
    return subprocess.check_output(command, text=True).strip()

artifacts = []
for path in paths:
    payload = (root / path).read_bytes()
    artifacts.append({
        "path": path.as_posix(),
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    })

manifest = {
    "schemaVersion": 1,
    "gitCommit": os.environ["RETE_SOURCE_REVISION"],
    "toolchain": {
        "rust": version("rustc", "--version"),
        "nightly": version(
            "rustup", "run", os.environ["ASYNCIFY_TOOLCHAIN"], "rustc", "--version"
        ),
        "wasmPack": version("wasm-pack", "--version"),
        "binaryen": version("wasm-opt", "--version"),
        "node": version("node", "--version"),
    },
    "artifacts": artifacts,
}
(root / "docs/wasm-build.json").write_text(
    json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
)
PY

echo ">> wrote docs/wasm-build.json for $RETE_SOURCE_REVISION"
