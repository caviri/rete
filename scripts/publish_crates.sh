#!/bin/sh
set -eu

cd "$(dirname "$0")/.."

MODE=${1:?expected --bootstrap, --trusted, or --verify-auth}
VERSION=${2:?expected release version}

case "$MODE" in
  --bootstrap|--trusted|--verify-auth) ;;
  *)
    echo "unsupported publish mode: $MODE" >&2
    exit 2
    ;;
esac

fail() {
  echo "publish preflight: $*" >&2
  exit 1
}

test "$(git status --porcelain)" = "" || fail "release worktree is dirty"

TAG=$(git describe --tags --exact-match 2>/dev/null || true)
test "$TAG" = "v$VERSION" || fail "HEAD is not tagged v$VERSION (found ${TAG:-no exact tag})"

if [ "$MODE" != "--verify-auth" ]; then
  test -n "${CARGO_REGISTRY_TOKEN:-}" || fail "CARGO_REGISTRY_TOKEN is required"
fi

REGISTRY_API=${RETE_REGISTRY_API:-https://crates.io/api/v1/crates}
REGISTRY_WEB=${RETE_REGISTRY_WEB:-https://crates.io/crates}
POLL_SECONDS=${RETE_POLL_SECONDS:-5}
POLL_ATTEMPTS=${RETE_POLL_ATTEMPTS:-120}
PACKAGES="rete-core rete-cli rete-wasm"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

cargo metadata --no-deps --format-version 1 > "$TMP/metadata.json"
python3 - "$TMP/metadata.json" "$VERSION" <<'PY'
import json
import sys

metadata_path, expected = sys.argv[1:]
with open(metadata_path, encoding="utf-8") as stream:
    metadata = json.load(stream)

names = ("rete-core", "rete-cli", "rete-wasm")
packages = {package["name"]: package for package in metadata["packages"]}
for name in names:
    package = packages.get(name)
    if package is None:
        raise SystemExit(f"missing workspace package {name}")
    if package["version"] != expected:
        raise SystemExit(
            f"{name} has version {package['version']}, expected {expected}"
        )

for name in ("rete-cli", "rete-wasm"):
    dependencies = [
        dependency
        for dependency in packages[name]["dependencies"]
        if dependency["name"] == "rete-core"
    ]
    if len(dependencies) != 1 or dependencies[0]["req"] != f"={expected}":
        found = dependencies[0]["req"] if dependencies else "missing"
        raise SystemExit(
            f"{name} must depend on rete-core ={expected}; found {found}"
        )
PY

if [ "$MODE" != "--verify-auth" ]; then
  echo "== strict publication security gate =="
  # Development CI temporarily ignores the two quick-xml findings. Registry
  # publication deliberately does not: an active advisory stops this script.
  cargo audit --deny warnings
  cargo deny check advisories bans licenses sources
fi

prepare_package() {
  name=$1
  archive="target/package/$name-$VERSION.crate"

  echo "== package $name $VERSION ==" >&2
  cargo package --locked -p "$name" >&2
  test -f "$archive" || fail "cargo package did not create $archive"

  if [ "$name" != "rete-core" ]; then
    python3 - "$archive" "$name" "$VERSION" <<'PY'
import sys
import tarfile
import tomllib

archive, name, version = sys.argv[1:]
member_name = f"{name}-{version}/Cargo.toml"
with tarfile.open(archive, "r:gz") as package:
    member = package.extractfile(member_name)
    if member is None:
        raise SystemExit(f"{archive} has no normalized Cargo.toml")
    manifest = tomllib.loads(member.read().decode("utf-8"))

dependency = manifest.get("dependencies", {}).get("rete-core")
if isinstance(dependency, str):
    requirement = dependency
elif isinstance(dependency, dict):
    requirement = dependency.get("version")
else:
    requirement = None

expected = f"={version}"
if requirement != expected:
    raise SystemExit(
        f"{name} packaged rete-core requirement is {requirement!r}, expected {expected!r}"
    )
PY
  fi

  sha256sum "$archive" | awk '{print $1}'
}

registry_checksum() {
  name=$1
  version=$2
  body="$TMP/registry-$name-$version.json"
  status=$(curl -sS -o "$body" -w '%{http_code}' \
    "$REGISTRY_API/$name/$version") || {
      echo "crates.io request failed for $name $version" >&2
      return 5
    }

  case "$status" in
    200)
      python3 - "$body" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    payload = json.load(stream)
checksum = payload.get("version", {}).get("checksum")
if not checksum:
    raise SystemExit("registry response has no version.checksum")
print(checksum)
PY
      ;;
    404)
      return 4
      ;;
    *)
      echo "crates.io returned HTTP $status for $name $version" >&2
      return 5
      ;;
  esac
}

wait_for_registry() {
  name=$1
  version=$2
  expected=$3
  attempt=1

  while [ "$attempt" -le "$POLL_ATTEMPTS" ]; do
    if remote=$(registry_checksum "$name" "$version"); then
      test "$remote" = "$expected" || \
        fail "$name $version became visible with a different checksum"
      if cargo info "$name@$version" >/dev/null 2>&1; then
        return 0
      fi
    else
      result=$?
      test "$result" -eq 4 || return "$result"
    fi

    sleep "$POLL_SECONDS"
    attempt=$((attempt + 1))
  done

  fail "$name $version was not visible through both the API and Cargo after $POLL_ATTEMPTS attempts"
}

append_receipt_row() {
  name=$1
  checksum=$2
  verified_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  printf '%s\t%s\t%s/%s/%s\t%s\n' \
    "$name" "$checksum" "$REGISTRY_WEB" "$name" "$VERSION" "$verified_at" \
    >> "$TMP/receipt.tsv"
}

publish_package() {
  name=$1
  local_checksum=$(prepare_package "$name")

  if remote_checksum=$(registry_checksum "$name" "$VERSION"); then
    test "$remote_checksum" = "$local_checksum" || \
      fail "$name $VERSION already exists with a different checksum"
    echo "== $name $VERSION already exists with the expected checksum =="
    wait_for_registry "$name" "$VERSION" "$local_checksum"
  else
    result=$?
    test "$result" -eq 4 || exit "$result"
    echo "== publish $name $VERSION =="
    cargo publish --dry-run --locked -p "$name"
    cargo publish --locked -p "$name"
    wait_for_registry "$name" "$VERSION" "$local_checksum"
  fi

  append_receipt_row "$name" "$local_checksum"
}

verify_registry_archives() {
  while IFS="	" read -r name expected _url _timestamp; do
    archive="$TMP/download-$name-$VERSION.crate"
    status=$(curl -sS -L -o "$archive" -w '%{http_code}' \
      "$REGISTRY_API/$name/$VERSION/download") || \
      fail "could not download the registry archive for $name $VERSION"
    test "$status" = 200 || \
      fail "registry archive download for $name $VERSION returned HTTP $status"
    actual=$(sha256sum "$archive" | awk '{print $1}')
    test "$actual" = "$expected" || \
      fail "downloaded $name $VERSION archive has a different checksum"
  done < "$TMP/receipt.tsv"
}

verify_fresh_consumers() {
  verify_root="$TMP/verify"
  cargo_home="$verify_root/cargo-home"
  mkdir -p "$cargo_home" "$verify_root/install" \
    "$verify_root/core/src" "$verify_root/wasm/src"

  CARGO_HOME="$cargo_home" cargo install rete-cli \
    --version "=$VERSION" --locked --root "$verify_root/install"

  cat > "$verify_root/core/Cargo.toml" <<EOF
[package]
name = "rete-core-release-consumer"
version = "0.0.0"
edition = "2021"

[dependencies]
rete-core = "=$VERSION"
EOF
  echo 'fn main() {}' > "$verify_root/core/src/main.rs"
  CARGO_HOME="$cargo_home" cargo check \
    --manifest-path "$verify_root/core/Cargo.toml"

  cat > "$verify_root/wasm/Cargo.toml" <<EOF
[package]
name = "rete-wasm-release-consumer"
version = "0.0.0"
edition = "2021"

[dependencies]
rete-wasm = "=$VERSION"
EOF
  echo '#![allow(dead_code)] pub fn consumer() {}' > "$verify_root/wasm/src/lib.rs"
  CARGO_HOME="$cargo_home" cargo check \
    --manifest-path "$verify_root/wasm/Cargo.toml" \
    --target wasm32-unknown-unknown
}

write_receipt() {
  receipt=target/release/crates-io-receipt.json
  mkdir -p "$(dirname "$receipt")"
  commit=$(git rev-parse HEAD)
  generated_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  python3 - "$TMP/receipt.tsv" "$receipt" "$VERSION" "$TAG" \
    "$commit" "$generated_at" <<'PY'
import csv
import json
import sys

rows_path, receipt_path, version, tag, commit, generated_at = sys.argv[1:]
packages = []
with open(rows_path, encoding="utf-8", newline="") as stream:
    for name, checksum, registry_url, verified_at in csv.reader(stream, delimiter="\t"):
        packages.append(
            {
                "package": name,
                "version": version,
                "checksum": checksum,
                "registry_url": registry_url,
                "verified_at": verified_at,
            }
        )

receipt = {
    "registry": "crates.io",
    "version": version,
    "tag": tag,
    "commit": commit,
    "generated_at": generated_at,
    "packages": packages,
}
with open(receipt_path, "w", encoding="utf-8") as stream:
    json.dump(receipt, stream, indent=2)
    stream.write("\n")
PY
  echo "wrote $receipt"
}

if [ "$MODE" = "--verify-auth" ]; then
  for name in $PACKAGES; do
    prepare_package "$name" >/dev/null
  done
  echo "authentication and package preflight succeeded; no crate was published"
  exit 0
fi

: > "$TMP/receipt.tsv"
for name in $PACKAGES; do
  publish_package "$name"
done

verify_registry_archives
verify_fresh_consumers
write_receipt
