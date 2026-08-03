#!/bin/sh
# Canonical release-candidate checks. Run inside the pinned Compose dev image:
#   docker compose run --rm check
# Individual CI jobs may select: quality, security, or docs.
set -eu

cd "$(dirname "$0")/.."

quality() {
  # Cheapest gate first: every client must sit on the engine's minor line.
  python3 scripts/sync_versions.py --check
  cargo fmt --all -- --check
  cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
  cargo test --workspace --exclude rete-bench
  cargo test -p rete-core --no-default-features
  cargo build -p rete-core --all-features
  cargo build -p rete-bench
  # `--exclude rete-bench` above keeps the SLOW benchmarks out of the gate, but it
  # was also excluding the differential oracle that lives in the same crate — a
  # correctness gate (its own words) that consequently never ran anywhere, while
  # build reports went on printing "differential oracle green". It costs 0.11 s.
  cargo test -p rete-bench --test differential
  bash scripts/smoke.sh
  cargo run --release -p rete-core --example qbench -- --check
}

security() {
  # These are the only accepted development exceptions. The crates.io publish
  # preflight deliberately runs cargo audit again without --ignore.
  cargo audit --deny warnings \
    --ignore RUSTSEC-2026-0194 \
    --ignore RUSTSEC-2026-0195
  cargo deny check advisories bans licenses sources
}

docs() {
  RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --exclude rete-bench --no-deps
  cargo run -q -p docgen
  git diff --exit-code -- docs/ ':!docs/playground.html' ':!docs/wasm-build.json' || {
    echo "generated docs are stale; run 'cargo run -q -p docgen' and commit them" >&2
    return 1
  }
  python3 scripts/check_docs_links.py
}

case "${1:-all}" in
  all)
    quality
    security
    docs
    ;;
  quality|security|docs)
    "$1"
    ;;
  *)
    echo "usage: $0 [all|quality|security|docs]" >&2
    exit 2
    ;;
esac
