#!/bin/sh
# Guard: in a wasm build, getrandom 0.3 may be reached ONLY by oxrdf 0.3's
# blank-node ids.
#
#   sh scripts/check_getrandom03.sh wasm   # the browser engine (crates/rete-wasm)
#   sh scripts/check_getrandom03.sh ffi    # the Chicory engine (clients/java/ffi)
#
# Why this exists. The RDF 1.2 Turtle/TriG reader (`oxttl` 0.2) brings
# `oxrdf` 0.3, which calls `rand::random()` in exactly one place,
# `src/blank_node.rs`, to label anonymous blank nodes. That needs UNIQUENESS, not
# secrecy, and the Chicory engine feeds getrandom 0.3 from a non-cryptographic
# xorshift on the strength of that (clients/java/ffi/src/lib.rs). The browser
# engine gets crypto.getRandomValues, but it is the same reasoning that let the
# backend be chosen without a security review.
#
# That reasoning holds only while blank-node labelling is the only consumer. The
# day a crate that wants real entropy (a hash seed, a nonce, a key) starts
# depending on getrandom 0.3 in these builds, it would silently get the
# xorshift. So this check compares the complete set of crates that depend on
# getrandom 0.3 in the wasm build against the known set, and fails, naming the
# newcomer, on any difference. Adding a crate to COMMON below is a security
# decision: say in the PR what that crate uses randomness for.
set -eu

cd "$(dirname "$0")/.."

# Every crate on a path to getrandom 0.3 today. `rand_chacha` is rand's own
# ThreadRng; `rete-core` is listed because it names getrandom 0.3 as a direct
# dependency so its `wasm-js` feature can pick the browser backend (its own code
# must not call it; see the source check below).
COMMON="getrandom oxrdf oxttl rand rand_chacha rand_core rete-core"

inverse_tree() { # inverse_tree <cargo tree args...>
  cargo tree --target wasm32-unknown-unknown -e normal --prefix none \
    --format '{p}' -i getrandom@0.3 "$@" |
    sed -e 's/ (\*)$//' -e 's/ (proc-macro)$//' |
    awk '{print $1}' | sort -u | tr '\n' ' ' | sed 's/ $//'
}

check() { # check <label> <expected crate names> <cargo tree args...>
  label=$1
  expected=$(printf '%s\n' $2 | sort -u | tr '\n' ' ' | sed 's/ $//')
  shift 2
  actual=$(inverse_tree "$@")
  if [ "$actual" != "$expected" ]; then
    tmp=$(mktemp -d)
    printf '%s\n' $expected > "$tmp/expected"
    printf '%s\n' $actual > "$tmp/actual"
    echo "check_getrandom03 FAILED ($label): the set of crates that depend on getrandom 0.3" >&2
    echo "in the wasm32 build changed." >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    echo "  new:      $(comm -13 "$tmp/expected" "$tmp/actual" | tr '\n' ' ')" >&2
    echo "  gone:     $(comm -23 "$tmp/expected" "$tmp/actual" | tr '\n' ' ')" >&2
    echo "Read the header of $0 before adding anything to COMMON." >&2
    rm -rf "$tmp"
    return 1
  fi
  echo "getrandom 0.3 in the $label wasm build: only the oxrdf blank-node path ($actual)."
}

# rete-core's own code must not draw from getrandom 0.3 directly: the direct
# dependency exists only to select a backend.
source_check() {
  if grep -rn 'getrandom03' crates/rete-core/src; then
    echo "check_getrandom03 FAILED: rete-core's source uses getrandom03 (above)." >&2
    echo "It is a dependency only to pick getrandom 0.3's wasm backend." >&2
    return 1
  fi
}

case "${1:-}" in
  wasm)
    source_check
    check rete-wasm "$COMMON rete-wasm" --locked -p rete-wasm
    ;;
  ffi)
    source_check
    # rete-ffi depends on getrandom 0.3 itself, to define __getrandom_v03_custom.
    check rete-ffi "$COMMON rete-ffi" --manifest-path clients/java/ffi/Cargo.toml
    ;;
  *)
    echo "usage: $0 wasm|ffi" >&2
    exit 2
    ;;
esac
