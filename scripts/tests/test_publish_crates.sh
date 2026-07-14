#!/bin/sh
set -eu

cd "$(dirname "$0")/../.."

SCRIPT="$PWD/scripts/publish_crates.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

FAKEBIN="$TMP/bin"
mkdir -p "$FAKEBIN"

cat > "$FAKEBIN/fake" <<'FAKE'
#!/bin/sh
set -eu

command_name=${0##*/}
printf '%s %s\n' "$command_name" "$*" >> "$FAKE_LOG"

checksum_for() {
  case "$1" in
    rete-core) printf '%064d\n' 1 ;;
    rete-cli) printf '%064d\n' 2 ;;
    rete-wasm) printf '%064d\n' 3 ;;
    *) echo "unknown package: $1" >&2; exit 1 ;;
  esac
}

package_arg() {
  previous=
  for argument in "$@"; do
    if [ "$previous" = "-p" ]; then
      printf '%s\n' "$argument"
      return
    fi
    previous=$argument
  done
  echo "missing -p package argument" >&2
  exit 1
}

case "$command_name" in
  git)
    case "${1:-}" in
      status)
        [ "${FAKE_DIRTY:-0}" = 1 ] && echo " M dirty.txt"
        ;;
      describe)
        printf '%s\n' "$FAKE_TAG"
        ;;
      rev-parse)
        printf '%s\n' "0123456789abcdef0123456789abcdef01234567"
        ;;
      *)
        echo "unexpected git invocation: $*" >&2
        exit 1
        ;;
    esac
    ;;
  cargo)
    subcommand=${1:-}
    shift || true
    case "$subcommand" in
      metadata)
        cat <<JSON
{"packages":[
  {"name":"rete-core","version":"$FAKE_VERSION","dependencies":[]},
  {"name":"rete-cli","version":"$FAKE_VERSION","dependencies":[{"name":"rete-core","req":"=$FAKE_VERSION"}]},
  {"name":"rete-wasm","version":"$FAKE_VERSION","dependencies":[{"name":"rete-core","req":"=$FAKE_VERSION"}]}
]}
JSON
        ;;
      package)
        name=$(package_arg "$@")
        package_root="target/package/$name-$FAKE_VERSION"
        rm -rf "$package_root"
        mkdir -p "$package_root/src"
        {
          echo '[package]'
          echo "name = \"$name\""
          echo "version = \"$FAKE_VERSION\""
          if [ "$name" != rete-core ]; then
            echo '[dependencies.rete-core]'
            echo "version = \"=$FAKE_VERSION\""
          fi
        } > "$package_root/Cargo.toml"
        echo 'fn main() {}' > "$package_root/src/main.rs"
        mkdir -p target/package
        tar -czf "target/package/$name-$FAKE_VERSION.crate" \
          -C target/package "$name-$FAKE_VERSION"
        ;;
      publish)
        name=$(package_arg "$@")
        case " $* " in
          *" --dry-run "*) ;;
          *)
            checksum=$(checksum_for "$name")
            printf '%s %s\n' "$name" "$checksum" >> "$FAKE_STATE"
            printf 'PUBLISHED %s\n' "$name" >> "$FAKE_LOG"
            ;;
        esac
        ;;
      info)
        spec=${1:?missing cargo info package}
        name=${spec%@*}
        grep -q "^$name " "$FAKE_STATE"
        ;;
      audit|deny|install|check)
        ;;
      *)
        echo "unexpected cargo invocation: $subcommand $*" >&2
        exit 1
        ;;
    esac
    ;;
  curl)
    output=
    url=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -o|--output|-w|--write-out)
          shift
          [ "$#" -gt 0 ] || exit 2
          [ "${1:-}" = '%{http_code}' ] || output=$1
          ;;
        -s|-S|-sS|-L|-f|--fail|--location)
          ;;
        http://*|https://*)
          url=$1
          ;;
      esac
      shift
    done
    path=${url#*/api/v1/crates/}
    name=${path%%/*}
    rest=${path#*/}
    version=${rest%%/*}
    if [ "$rest" != "$version" ] && [ "${rest#*/}" = download ]; then
      if grep -q "^$name " "$FAKE_STATE"; then
        printf 'registry archive for %s\n' "$name" > "$output"
        printf 200
      else
        : > "$output"
        printf 404
      fi
    elif checksum=$(awk -v name="$name" '$1 == name { print $2; found=1 } END { if (!found) exit 1 }' "$FAKE_STATE"); then
      printf '{"version":{"checksum":"%s"}}\n' "$checksum" > "$output"
      printf 200
    else
      : > "$output"
      printf 404
    fi
    ;;
  sha256sum)
    file=${1:?missing checksum file}
    case "$file" in
      *rete-core*) name=rete-core ;;
      *rete-cli*) name=rete-cli ;;
      *rete-wasm*) name=rete-wasm ;;
      *)
        name=$(sed -n 's/^registry archive for \(rete-[a-z]*\)$/\1/p' "$file")
        ;;
    esac
    checksum=$(checksum_for "$name")
    printf '%s  %s\n' "$checksum" "$file"
    ;;
  *)
    echo "unexpected fake command: $command_name" >&2
    exit 1
    ;;
esac
FAKE

chmod +x "$FAKEBIN/fake"
for command_name in cargo curl git sha256sum; do
  cp "$FAKEBIN/fake" "$FAKEBIN/$command_name"
done

passes=0
fails=0

pass() {
  passes=$((passes + 1))
  printf 'ok   %s\n' "$1"
}

fail() {
  fails=$((fails + 1))
  printf 'FAIL %s\n' "$1" >&2
}

prepare_case() {
  case_name=$1
  case_dir="$TMP/$case_name"
  rm -rf "$case_dir"
  mkdir -p "$case_dir"
  export FAKE_LOG="$case_dir/invocations.log"
  export FAKE_STATE="$case_dir/registry.state"
  export FAKE_VERSION="${2:-1.0.0-rc.1}"
  export FAKE_TAG="v$FAKE_VERSION"
  export FAKE_DIRTY=0
  : > "$FAKE_LOG"
  : > "$FAKE_STATE"
}

run_publish() {
  PATH="$FAKEBIN:$PATH" \
  RETE_REGISTRY_API="https://registry.test/api/v1/crates" \
  RETE_POLL_SECONDS=0 \
  RETE_POLL_ATTEMPTS=2 \
  CARGO_REGISTRY_TOKEN="${CARGO_REGISTRY_TOKEN:-}" \
    sh "$SCRIPT" "$@"
}

prepare_case tag_mismatch
FAKE_TAG=v1.0.0-rc.2
export FAKE_TAG
CARGO_REGISTRY_TOKEN=test-token
export CARGO_REGISTRY_TOKEN
if run_publish --bootstrap 1.0.0-rc.1 > "$case_dir/output" 2>&1; then
  fail "tag/version mismatch is rejected"
else
  pass "tag/version mismatch is rejected"
fi

prepare_case dirty
FAKE_DIRTY=1
export FAKE_DIRTY
CARGO_REGISTRY_TOKEN=test-token
export CARGO_REGISTRY_TOKEN
if run_publish --bootstrap 1.0.0-rc.1 > "$case_dir/output" 2>&1; then
  fail "dirty worktree is rejected"
else
  pass "dirty worktree is rejected"
fi

prepare_case missing_token
unset CARGO_REGISTRY_TOKEN
if run_publish --bootstrap 1.0.0-rc.1 > "$case_dir/output" 2>&1; then
  fail "bootstrap without a token is rejected"
else
  pass "bootstrap without a token is rejected"
fi

prepare_case ordered
CARGO_REGISTRY_TOKEN=test-token
export CARGO_REGISTRY_TOKEN
if run_publish --bootstrap 1.0.0-rc.1 > "$case_dir/output" 2>&1; then
  actual=$(sed -n 's/^PUBLISHED //p' "$FAKE_LOG" | tr '\n' ' ' | sed 's/ $//')
  if [ "$actual" = "rete-core rete-cli rete-wasm" ]; then
    pass "publishes core, CLI, and WASM in dependency order"
  else
    fail "publication order was '$actual'"
  fi
  if [ -f target/release/crates-io-receipt.json ] && \
      ! grep -q 'test-token' target/release/crates-io-receipt.json; then
    pass "writes a non-secret publication receipt"
  else
    fail "publication receipt is missing or contains the token"
  fi
else
  fail "ordered bootstrap succeeds: $(tail -1 "$case_dir/output")"
fi

prepare_case resume 1.0.0-rc.2
printf 'rete-core %064d\n' 1 > "$FAKE_STATE"
CARGO_REGISTRY_TOKEN=oidc-token
export CARGO_REGISTRY_TOKEN
if run_publish --trusted 1.0.0-rc.2 > "$case_dir/output" 2>&1; then
  actual=$(sed -n 's/^PUBLISHED //p' "$FAKE_LOG" | tr '\n' ' ' | sed 's/ $//')
  if [ "$actual" = "rete-cli rete-wasm" ]; then
    pass "trusted publishing resumes after an existing core crate"
  else
    fail "trusted resume published '$actual'"
  fi
else
  fail "trusted resume succeeds: $(tail -1 "$case_dir/output")"
fi

prepare_case checksum_mismatch 1.0.0-rc.2
printf 'rete-core %064d\n' 9 > "$FAKE_STATE"
CARGO_REGISTRY_TOKEN=oidc-token
export CARGO_REGISTRY_TOKEN
if run_publish --trusted 1.0.0-rc.2 > "$case_dir/output" 2>&1; then
  fail "existing checksum mismatch is rejected"
else
  pass "existing checksum mismatch is rejected"
fi

prepare_case verify_auth 1.0.0-rc.2
unset CARGO_REGISTRY_TOKEN
if run_publish --verify-auth 1.0.0-rc.2 > "$case_dir/output" 2>&1; then
  if grep -q '^PUBLISHED ' "$FAKE_LOG"; then
    fail "authentication-only mode published a crate"
  else
    pass "authentication-only mode never publishes"
  fi
else
  fail "authentication-only mode succeeds: $(tail -1 "$case_dir/output")"
fi

printf '%s passed; %s failed\n' "$passes" "$fails"
test "$fails" -eq 0
