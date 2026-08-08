#!/usr/bin/env bash
# THE producer of the gate's .rete fixtures. One script, used by gate.sh,
# scripts/build_wasm.sh and CI alike — because three producers is how the three
# copies drifted apart in the first place.
#
#   bash tests/gate/fixtures.sh            # build (if stale) + verify
#   bash tests/gate/fixtures.sh --force    # rebuild unconditionally
#   bash tests/gate/fixtures.sh --verify   # verify what is on disk, build nothing
#
# What it guarantees, and why each half exists:
#
#  1. EVERY fixture is BUILT from a tracked source in tests/gate/fixtures/.
#     Nothing is downloaded. gate.sh used to fetch tests/gate/.cache/worldcup2026.rete
#     from R2 when it was missing — but the published dataset of that name is a
#     different graph (16,184 triples, WITH a Dataset Card) while the recipe
#     builds a 7-triple CARDLESS file, and check_card_modal asserts cardless.
#     A fresh clone therefore could not get a green gate from gate.sh alone.
#
#  2. The rete-cli that builds them is CAPABILITY-CHECKED first. A binary older
#     than b7652657 (PR #161, 2026-08-04) accepts a card file carrying
#     version/creators/publisher/doi/cite_as/keywords/theme/extra/canonical_url/
#     sparql_endpoint/derived_from/source_date, exits 0, prints a plausible
#     "embedded dataset card (N bytes of metadata)" — and writes none of them,
#     nor any build record. Verified on four such binaries: build exit 0, twelve
#     fields gone. The gate then reddens check_card_modal, blaming the
#     playground for the binary's age.
#
#  3. Each produced file is VERIFIED against the properties its consumers
#     depend on (tests/gate/fixtures/manifest.json): quad count, named-graph
#     count, card present/absent, build record, curated fields. A fixture that
#     came out wrong fails HERE, naming itself and the command to fix it —
#     never by making an unrelated check go red.
#
# Runs the toolchain in Docker when cargo is not on PATH (the host case), and
# natively when it is (inside the devcontainer, and in CI).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

MODE=build
for a in "$@"; do
  case "$a" in
    --force) MODE=force ;;
    --verify) MODE=verify ;;
    -h|--help) sed -n '2,8p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "tests/gate/fixtures.sh: unknown argument '$a'" >&2; exit 2 ;;
  esac
done

# --- toolchain: native when cargo is here, Docker otherwise -------------------
# The recursion guard matters: if the container also lacks cargo we must fail
# with that fact, not spawn containers forever.
if ! command -v cargo >/dev/null 2>&1; then
  if [ -n "${RETE_FIXTURES_IN_DOCKER:-}" ]; then
    echo "tests/gate/fixtures.sh: no cargo inside the dev container — the image is broken." >&2
    exit 2
  fi
  command -v docker >/dev/null 2>&1 || {
    echo "tests/gate/fixtures.sh: needs either cargo on PATH or Docker." >&2
    exit 2
  }
  echo "── gate fixtures: no cargo on PATH → building in the dev container ──"
  # Compose resolves the project name from $COMPOSE_PROJECT_NAME, else from
  # `name: rete` in compose.yaml — so from a git WORKTREE this writes into the
  # same rete_cargo-target volume as every other checkout. Export
  # COMPOSE_PROJECT_NAME per worktree (compose.yaml, tests/gate/README.md).
  echo "   compose project: ${COMPOSE_PROJECT_NAME:-rete (shared by every worktree)}"
  export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'
  exec docker compose run --rm -T -e RETE_FIXTURES_IN_DOCKER=1 dev \
    bash tests/gate/fixtures.sh "$@"
fi

# RETE_CLI lets a caller that already has a binary skip the cargo round trip.
# It is not a trust escape hatch: whatever it points at still has to pass the
# capability probe below, which is precisely the check a hand-picked stale
# binary fails.
if [ -z "${RETE_CLI:-}" ]; then
  if [ "$MODE" != verify ]; then
    cargo build --release -q -p rete-cli
  fi
  TARGET_DIR="$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null \
    || echo "${CARGO_TARGET_DIR:-$ROOT/target}")"
  RETE_CLI="$TARGET_DIR/release/rete"
fi

export RETE_BIN="$RETE_CLI" RETE_MODE="$MODE"
python3 -P - <<'PY'
"""Build and verify the gate fixtures from tests/gate/fixtures/manifest.json."""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path.cwd()
SRC = ROOT / "tests/gate/fixtures"
MANIFEST = SRC / "manifest.json"
RETE = os.environ["RETE_BIN"]
MODE = os.environ["RETE_MODE"]

REBUILD = "rebuild them with:  bash tests/gate/fixtures.sh --force"
STALE_CLI = (
    "the rete-cli that produced it predates b7652657 (PR #161, 2026-08-04), when the "
    "curated identity fields and the kind-7 build record were added.\n"
    "  Such a binary accepts the card file, exits 0 and prints a card size — it just "
    "writes none of it.\n"
    "  Fix:  cargo build --release -p rete-cli   (host: docker compose run --rm dev "
    "cargo build --release -p rete-cli)"
)

if not Path(RETE).exists():
    sys.exit(f"tests/gate/fixtures.sh: no rete-cli at {RETE} — drop --verify to build one.")

manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
OUT = ROOT / manifest["outDir"]
OUT.mkdir(parents=True, exist_ok=True)
problems = []


def run(*argv):
    p = subprocess.run([RETE, *argv], capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build(fx, out_path):
    argv = ["build", str(SRC / fx["source"]), "-o", str(out_path)]
    if fx.get("cardFile"):
        argv += ["--card-file", str(SRC / fx["cardFile"])]
    argv += fx.get("extraArgs", [])
    code, _, err = run(*argv)
    if code != 0:
        problems.append(
            f"{fx['out']}: `rete {' '.join(argv)}` exited {code}\n  {err.strip()[:400]}"
        )
        return False
    return True


def observe(path):
    """Everything the manifest can assert, read back off the produced file."""
    obs = {}
    code, out, _ = run("info", str(path))
    if code != 0:
        return None
    for line in out.splitlines():
        line = line.strip().rstrip(",")
        for key in ("quad_count", "build_info_len"):
            if line.startswith(key + ":"):
                obs[key] = int(line.split(":", 1)[1].strip())
    # `rete graphs` lists one IRI per line, or prints the prose
    # "(default graph only — no named graphs)" — count only the IRIs.
    code, out, _ = run("graphs", str(path))
    obs["namedGraphs"] = len([l for l in out.splitlines() if l.startswith("<")]) if code == 0 else -1
    code, out, _ = run("sparql", str(path), "ASK { ?s ?p ?o }")
    obs["defaultGraphNonEmpty"] = out.strip() == "true" if code == 0 else None
    code, out, _ = run("card", "--json", str(path))
    obs["card"] = json.loads(out) if code == 0 and out.lstrip().startswith("{") else None
    return obs


def verify(fx, path):
    """Check one produced file against the properties its consumers rely on.

    Every failure names the FIXTURE and what to run — the whole point. A missing
    curated field additionally names the binary, because a stale rete-cli is the
    only cause that has ever produced that shape.
    """
    want = fx["assert"]
    name = fx["out"]
    users = "; ".join(fx.get("usedBy", [])) or "the gate"
    if not path.exists():
        problems.append(f"{name}: missing from {OUT.relative_to(ROOT)} — {REBUILD}")
        return None
    obs = observe(path)
    if obs is None:
        problems.append(
            f"{name}: `rete info` cannot read it — truncated, or written by an "
            f"incompatible rete-cli.\n  {REBUILD}"
        )
        return None

    ok = [True]

    def fail(msg):
        ok[0] = False
        problems.append(f"{name}: {msg}\n  needed by: {users}")

    if obs.get("quad_count") != want["quads"]:
        fail(f"holds {obs.get('quad_count')} quads, the recipe says {want['quads']} — {REBUILD}")
    if obs["namedGraphs"] != want["namedGraphs"]:
        fail(f"has {obs['namedGraphs']} named graph(s), the recipe says {want['namedGraphs']} — {REBUILD}")
    if want.get("defaultGraphTriples") == 0 and obs["defaultGraphNonEmpty"]:
        fail(f"its default graph is NOT empty; the recipe puts every quad in a named graph — {REBUILD}")

    card = obs["card"]
    if want["card"] and card is None:
        fail(f"carries NO Dataset Card, but was built with --card-file {fx['cardFile']}.\n  {STALE_CLI}")
        return obs
    if not want["card"] and card is not None:
        fail(
            "carries a Dataset Card, and the checks that read it assert it does NOT.\n"
            "  A downloaded or hand-placed file has replaced the built one: delete it and "
            f"{REBUILD}"
        )
        return obs

    if card is not None:
        if want.get("cardTitle") and card.get("title") != want["cardTitle"]:
            fail(f"card title is {card.get('title')!r}, its card file says {want['cardTitle']!r} — {REBUILD}")
        missing = [f for f in want.get("curatedFields", []) if not card.get(f)]
        if missing:
            fail(f"its card is missing the curated field(s) {', '.join(missing)}.\n  {STALE_CLI}")
        present = [f for f in want.get("absentCuratedFields", []) if card.get(f)]
        if present:
            fail(
                f"its card carries {', '.join(present)} — this fixture is the NEGATIVE half of a "
                "pair and must carry none of them, or check_card_modal stops testing that an "
                f"absent field renders as absent.\n  Edit {fx['cardFile']}, then {REBUILD}"
            )
        record = card.get("build")
        if want.get("buildRecord") and not record:
            fail(f"carries no build record (kind-7 section).\n  {STALE_CLI}")
        elif record:
            builder = record.get("builder") or ""
            prefix = want.get("buildRecordBuilderPrefix")
            if prefix and not builder.startswith(prefix):
                fail(f"its build record names the builder {builder!r}, expected one starting {prefix!r} — {REBUILD}")
            if want.get("buildRecordHasQueryCosts") and not (record.get("query_costs") or {}).get("queries"):
                fail(f"its build record has no measured starter-query costs, which check_card_modal renders — {REBUILD}")
    if want.get("buildRecord") is False and obs.get("build_info_len"):
        fail(f"carries a build record and the recipe says it must not — {REBUILD}")
    return obs if ok[0] else None


# --- capability probe: reject a stale binary BEFORE it writes anything --------
# Builds the full-card recipe into a throwaway path (7 triples, ~50 ms) and
# checks the two things a pre-#161 binary drops in silence. Doing it on a temp
# file is what makes the failure say "your rete-cli is old" instead of leaving a
# plausible-looking bad fixture in .cache for a browser check to trip over ten
# minutes later.
probe_src = next(f for f in manifest["fixtures"] if f["assert"].get("curatedFields"))
if MODE != "verify":
    tmp = Path(tempfile.mkdtemp(prefix="rete-fixture-probe-"))
    try:
        probe = tmp / "probe.rete"
        code, _, err = run(
            "build", str(SRC / probe_src["source"]),
            "--card-file", str(SRC / probe_src["cardFile"]), "-o", str(probe),
        )
        if code != 0:
            sys.exit(f"✗ rete-cli cannot build the probe fixture (exit {code}):\n  {err.strip()[:400]}")
        pcode, pout, _ = run("card", "--json", str(probe))
        pcard = json.loads(pout) if pcode == 0 and pout.lstrip().startswith("{") else {}
        gone = [f for f in probe_src["assert"]["curatedFields"] if not pcard.get(f)]
        builder = (pcard.get("build") or {}).get("builder")
        if gone or not builder:
            sys.exit(
                "✗ the rete-cli in this checkout cannot write the gate's fixtures.\n"
                f"    binary                : {RETE}\n"
                f"    build record          : {builder or 'ABSENT'}\n"
                f"    curated fields dropped: {', '.join(gone) or '(none)'}\n"
                f"  {STALE_CLI}"
            )
        print(f"── gate fixtures: builder {builder} ──")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

# --- build (unless --verify), then verify, then stamp -------------------------
manifest_hash = hashlib.sha256(MANIFEST.read_bytes()).hexdigest()
stamp_path = OUT / "fixtures.stamp.json"
prior = {}
if stamp_path.exists():
    try:
        prior = json.loads(stamp_path.read_text(encoding="utf-8"))
    except ValueError:
        prior = {}

sources = {}
for fx in manifest["fixtures"]:
    for key in ("source", "cardFile"):
        if fx.get(key):
            sources[fx[key]] = sha256(SRC / fx[key])

fresh = (
    MODE != "force"
    and prior.get("manifestSha256") == manifest_hash
    and prior.get("sourceSha256") == sources
    and all(
        (OUT / fx["out"]).exists()
        and sha256(OUT / fx["out"]) == (prior.get("fixtures", {}).get(fx["out"]) or {}).get("sha256")
        for fx in manifest["fixtures"]
    )
)

if fresh:
    print("── gate fixtures: up to date (recipe, sources and files unchanged) ──")
elif MODE != "verify":
    for fx in manifest["fixtures"]:
        out_path = OUT / fx["out"]
        out_path.unlink(missing_ok=True)
        build(fx, out_path)

stamp = {
    "schemaVersion": 1,
    "producer": "tests/gate/fixtures.sh",
    "manifestSha256": manifest_hash,
    "sourceSha256": sources,
    "fixtures": {},
}
for fx in manifest["fixtures"]:
    out_path = OUT / fx["out"]
    obs = verify(fx, out_path)
    if obs is not None:
        stamp["fixtures"][fx["out"]] = {
            "sha256": sha256(out_path),
            "bytes": out_path.stat().st_size,
            "builder": ((obs["card"] or {}).get("build") or {}).get("builder"),
            "card": obs["card"] is not None,
        }

if problems:
    print("\n✗ the gate's fixtures are not what the checks that read them require:\n", file=sys.stderr)
    for p in problems:
        print(f"  ✗ {p}\n", file=sys.stderr)
    sys.exit(1)

if not fresh and MODE != "verify":
    stamp_path.write_text(json.dumps(stamp, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    for name, meta in sorted(stamp["fixtures"].items()):
        print(f"   ✓ {name:<32} {meta['bytes']:>7} B  {'carded' if meta['card'] else 'cardless'}")
print(f"── gate fixtures: {len(manifest['fixtures'])} verified against tests/gate/fixtures/manifest.json ──")
PY
