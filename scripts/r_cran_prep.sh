#!/bin/bash
# Stage the R client as a standalone (CRAN-shaped) source package.
#
# The crate at clients/r/src/rust depends on rete-core by a path that climbs
# out of the package (../../../../crates/rete-core), so a plain `R CMD build`
# tarball cannot compile outside the monorepo. This script materializes that
# dependency inside the package: `cargo package -p rete-core` resolves the
# workspace-inherited fields into a standalone crate, which is embedded at
# src/rust/rete-core and the dependency re-pointed at it.
#
# Usage: r_cran_prep.sh <staging-dir> [--vendor]
#   <staging-dir>/rete     the CRAN-shaped package source
#   --vendor               additionally vendor all registry crates for a
#                          fully offline build (required for the actual CRAN
#                          submission; needs the network once, here)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="${1:?usage: r_cran_prep.sh <staging-dir> [--vendor]}"
VENDOR="${2:-}"

mkdir -p "$STAGE"
rm -rf "$STAGE/rete"
cp -a "$ROOT/clients/r" "$STAGE/rete"
rm -rf "$STAGE/rete/src/rust/target" "$STAGE/rete/src/rust/vendor" \
       "$STAGE/rete/src/.cargo" "$STAGE/rete/src/Makevars" "$STAGE/rete/src/Makevars.win"

echo "=== materializing rete-core (cargo package resolves workspace fields) ==="
# A private target dir: the repo's own target/ may be a bind mount with
# foreign-platform artifacts (Docker on Windows), which breaks cargo package.
PKG_TARGET="$(mktemp -d)"
(cd "$ROOT" && CARGO_TARGET_DIR="$PKG_TARGET" cargo package -p rete-core --no-verify --allow-dirty >/dev/null)
CRATE_TARBALL="$(ls -t "$PKG_TARGET"/package/rete-core-*.crate | head -1)"
mkdir -p "$STAGE/rete/src/rust/rete-core"
tar xzf "$CRATE_TARBALL" -C "$STAGE/rete/src/rust/rete-core" --strip-components=1
# .cargo-ok / .cargo_vcs_info are cargo-package bookkeeping, not sources.
rm -f "$STAGE/rete/src/rust/rete-core/.cargo-ok" \
      "$STAGE/rete/src/rust/rete-core/.cargo_vcs_info.json" \
      "$STAGE/rete/src/rust/rete-core/Cargo.toml.orig"

sed -i "s|path = '../../../../crates/rete-core'|path = 'rete-core'|" \
    "$STAGE/rete/src/rust/Cargo.toml"
grep -q "path = 'rete-core'" "$STAGE/rete/src/rust/Cargo.toml" || {
    echo "ERROR: failed to re-point the rete-core dependency" >&2
    exit 1
}

if [ "$VENDOR" = "--vendor" ]; then
    echo "=== vendoring registry crates for offline builds ==="
    (cd "$STAGE/rete/src/rust" && cargo vendor --locked ../vendor-src >/dev/null 2>vendor.log) || {
        cat "$STAGE/rete/src/rust/vendor.log" >&2
        exit 1
    }
    # Package the vendor tree the way the rextendr Makevars expects it:
    # src/rust/vendor.tar.xz unpacks to src/vendor, with the source
    # replacement in src/rust/vendor-config.toml.
    (cd "$STAGE/rete/src" && mv vendor-src vendor && \
        tar cJf rust/vendor.tar.xz vendor && rm -rf vendor)
    cat > "$STAGE/rete/src/rust/vendor-config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "../vendor"
EOF
    rm -f "$STAGE/rete/src/rust/vendor.log"
fi

echo "=== staged: $STAGE/rete ==="
du -sh "$STAGE/rete" "$STAGE/rete/src/rust/rete-core" 2>/dev/null || true
echo "Next: R CMD build $STAGE/rete && R CMD check --as-cran rete_*.tar.gz"
