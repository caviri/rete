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
command -v node >/dev/null || {
  echo "node is required (use the devcontainer image)" >&2
  exit 1
}

if [[ -z "${RETE_SOURCE_REVISION:-}" ]]; then
  if [[ -n "${GITHUB_SHA:-}" ]]; then
    RETE_SOURCE_REVISION="$GITHUB_SHA"
  # `-c safe.directory=*`: the checkout is a bind mount, so inside the container
  # /work is owned by a uid git does not recognize and it refuses the repository
  # as "dubious ownership" — which made `docker compose run --rm wasm` fail on a
  # FRESH CLONE with "cannot resolve the source revision", a message about the
  # wrong thing entirely. Scoped to this one read-only command rather than
  # written into the container's global config.
  elif RETE_SOURCE_REVISION="$(git -c safe.directory='*' rev-parse HEAD 2>/dev/null)"; then
    :
  else
    # What is left is a real git WORKTREE: its .git is a file pointing at a host
    # path the container cannot see, so no ownership exception can help.
    echo "cannot resolve the source revision: /work is a git worktree whose .git file" >&2
    echo "points outside the mount (or is not a repository at all)." >&2
    echo "pass: docker compose run -e RETE_SOURCE_REVISION=\$(git rev-parse HEAD) --rm wasm" >&2
    exit 1
  fi
fi
export RETE_SOURCE_REVISION

# The build stamp lands in docs/playground.html (the topbar badge and
# window.RETE_BUILD), which is TRACKED and byte-diffed by CI's parity job. The
# workspace version is what belongs there, and CI passes no stamp at all — it
# reruns this script — so this default IS the value CI's rebuild carries. One
# derivation, which is the point: ci.yml used to compute it separately, and a
# separate correct value is what let the wrong default here go unnoticed.
#
# It used to default to $RETE_SOURCE_REVISION. Getting past the worktree guard
# above requires passing RETE_SOURCE_REVISION, and passing only that stamped the
# page with a 40-character SHA where CI writes `0.3.2`: two lines of diff, one
# env var, one wasted CI cycle (#199). An explicit RETE_BUILD_STAMP still wins,
# because two workflows set one deliberately and neither commits the page —
# release.yml stamps the release version, and the PR-preview job stamps the
# 12-character head SHA.
RETE_WORKSPACE_VERSION="$(python3 -P -c "import re,pathlib;print(re.search(r'(?ms)^\[workspace\.package\].*?^version = \"([^\"]+)\"', pathlib.Path('Cargo.toml').read_text()).group(1))")"
if [[ -z "${RETE_BUILD_STAMP:-}" ]]; then
  RETE_BUILD_STAMP="$RETE_WORKSPACE_VERSION"
  RETE_BUILD_STAMP_NOTE="default: the workspace version, the string CI stamps"
elif [[ "$RETE_BUILD_STAMP" == "$RETE_WORKSPACE_VERSION" ]]; then
  RETE_BUILD_STAMP_NOTE="explicit, and equal to the workspace version"
else
  RETE_BUILD_STAMP_NOTE="EXPLICIT, and NOT the workspace version $RETE_WORKSPACE_VERSION
                 -> docs/playground.html will NOT match CI's parity rebuild.
                 Right for a release or a PR preview, neither of which commits
                 the page. Otherwise unset RETE_BUILD_STAMP and let it default."
fi
export RETE_BUILD_STAMP

# The wasm builds get target dirs of their own — never one shared with a host
# build. Full reasoning in the file itself.
source scripts/wasm_target_dir.sh

echo ">> source revision : $RETE_SOURCE_REVISION"
echo ">> build stamp     : $RETE_BUILD_STAMP  ($RETE_BUILD_STAMP_NOTE)"
echo ">> wasm target dir : $RETE_WASM_TARGET_DIR"
echo ">> asyncify tgt dir: $RETE_WASM_ASYNC_TARGET_DIR"

# Binaryen v108 is intentionally retained for the reference-types-disabled
# Asyncify pass. It corrupts modern wasm-bindgen externref tables, so regular
# builds explicitly skip wasm-opt. Rust's release profile still optimizes them.
CARGO_TARGET_DIR="$RETE_WASM_TARGET_DIR" \
  wasm-pack build crates/rete-wasm --target web --out-dir ../../web/pkg --no-opt
CARGO_TARGET_DIR="$RETE_WASM_TARGET_DIR" \
  wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules --no-opt
node tests/gate/checks/check_wasm_boot.mjs
# docs/engine/ is the tracked ESM copy the standalone docs pages import
# (anatomy/bim-pair/building). It used to be a hand-copy with no producer —
# refresh it here so the CI parity diff below can actually guard it.
mkdir -p docs/engine
cp web/pkg/rete_wasm.js web/pkg/rete_wasm_bg.wasm docs/engine/
bash scripts/build_playground_async.sh
uv run python scripts/stage_playground_datasets.py
uv run python scripts/build_playground.py
# The gate's .rete fixtures. This used to be five inline `cargo run` lines here,
# five more in .github/workflows/ci.yml, and a curl in tests/gate/gate.sh — three
# producers of the same five files, which is how they drifted. One producer now,
# with the recipe and the asserted properties of each fixture in
# tests/gate/fixtures/manifest.json, and a capability check on the rete-cli that
# writes them (a stale binary silently drops every curated card field).
bash tests/gate/fixtures.sh

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
    # The string stamped into docs/playground.html. Recorded because getting it
    # wrong is invisible until CI byte-diffs the page (#199).
    "buildStamp": os.environ["RETE_BUILD_STAMP"],
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
