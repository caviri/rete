# shellcheck shell=bash
# Sourced (never executed) by scripts/build_wasm.sh and
# scripts/build_playground_async.sh, from the repository root. Sets
# RETE_WASM_PACK_MODE for their `wasm-pack build --mode …` calls.
#
# The wasm-bindgen CLI must be EXACTLY the wasm-bindgen version in Cargo.lock
# (the generated glue and the wasm's custom section are a matched pair).
# wasm-pack's default is to download such a CLI from GitHub on the first build
# in a fresh container. That made the build depend on the network and on what
# the download served, and one agent could not build at all. The dev image now
# carries the pinned CLI (.devcontainer/Dockerfile, WASM_BINDGEN_VERSION), and
# the builds run `--mode no-install`, so wasm-pack never fetches anything: a
# missing or mismatched CLI stops the build here, naming both versions.
#
# RETE_WASM_ALLOW_DOWNLOAD=1 restores the old behaviour (`--mode normal`) for a
# machine that cannot carry the CLI.

if [[ -z "${RETE_WASM_PACK_MODE:-}" ]]; then
  WASM_BINDGEN_LOCKED="$(awk '/^name = "wasm-bindgen"$/ { getline; if ($1 == "version") { gsub(/"/, "", $3); print $3; exit } }' Cargo.lock)"
  if [[ -z "$WASM_BINDGEN_LOCKED" ]]; then
    echo "cannot read the wasm-bindgen version from Cargo.lock" >&2
    exit 1
  fi
  WASM_BINDGEN_FOUND="$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }' || true)"
  if [[ "$WASM_BINDGEN_FOUND" == "$WASM_BINDGEN_LOCKED" ]]; then
    RETE_WASM_PACK_MODE=no-install
  elif [[ "${RETE_WASM_ALLOW_DOWNLOAD:-}" == "1" ]]; then
    echo ">> wasm-bindgen CLI ${WASM_BINDGEN_FOUND:-absent}, Cargo.lock needs $WASM_BINDGEN_LOCKED;" \
      "RETE_WASM_ALLOW_DOWNLOAD=1, so wasm-pack may download it" >&2
    RETE_WASM_PACK_MODE=normal
  else
    {
      echo "wasm-bindgen CLI: found ${WASM_BINDGEN_FOUND:-none}, Cargo.lock needs $WASM_BINDGEN_LOCKED."
      echo "  dev image: rebuild it (docker compose build dev). The CLI is pinned by"
      echo "    WASM_BINDGEN_VERSION in .devcontainer/Dockerfile, which must equal Cargo.lock."
      echo "  elsewhere: cargo install wasm-bindgen-cli --version $WASM_BINDGEN_LOCKED --locked"
      echo "  or set RETE_WASM_ALLOW_DOWNLOAD=1 to let wasm-pack fetch it, as it used to."
    } >&2
    exit 1
  fi
  echo ">> wasm-bindgen    : ${WASM_BINDGEN_FOUND:-absent} (Cargo.lock $WASM_BINDGEN_LOCKED), wasm-pack --mode $RETE_WASM_PACK_MODE"
fi
export RETE_WASM_PACK_MODE
