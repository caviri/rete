#!/usr/bin/env bash
# The rete half of tests/interop/oxigraph.sh — everything that runs INSIDE the
# repo's dev container. Kept as a file rather than a heredoc so the caller can
# launch it with either `docker compose run --rm dev` (local) or a plain
# `docker run <pinned image>` (CI), and so it is reviewable on its own.
#
#   bash tests/interop/rete_side.sh export     # build + export every fixture
#   bash tests/interop/rete_side.sh rebuild    # rebuild from Oxigraph's dump
#
# Every command's stdout, stderr and exit code are written next to each other in
# dev/interop/, so the assertions in oxigraph.sh read evidence instead of
# re-running anything — and a failure can be inspected after the fact.
set -u

STAGE="${1:-export}"
cd "$(dirname "$0")/../.." || exit 2
ROOT="$PWD"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/rete"
OUT="$ROOT/dev/interop"
mkdir -p "$OUT"

cargo build --release -q -p rete-cli || exit 90
cd "$OUT" || exit 2

# run NAME -- CMD...  →  NAME.out, NAME.err, NAME.code
run() {
  local name="$1"; shift; [ "$1" = "--" ] && shift
  "$@" >"$name.out" 2>"$name.err"
  echo $? >"$name.code"
}

case "$STAGE" in
  export)
    run build_repairable    -- "$BIN" build repairable.nt   -o repairable.rete
    run build_strict        -- "$BIN" build repairable.nt   -o strict.rete --strict
    run build_unrepairable  -- "$BIN" build unrepairable.nt -o unrepairable.rete
    run build_named         -- "$BIN" build named.nq        -o named.rete
    run build_quoted        -- "$BIN" build quoted.nq       -o quoted.rete
    run validate_repairable -- "$BIN" validate repairable.nt

    run export_raw   -- "$BIN" export repairable.rete   --format nq
    run export_clean -- "$BIN" export repairable.rete   --format nq --sanitize-iris
    run export_unrep -- "$BIN" export unrepairable.rete --format nq --sanitize-iris
    run export_named -- "$BIN" export named.rete        --format nq

    # Both quoted-triple surfaces, from one file: the default RDF 1.2 triple
    # term, which this store reads, and the RDF-star surface it does not. The
    # negative one is what makes the positive one mean something.
    run export_quoted      -- "$BIN" export quoted.rete --format nq
    run export_quoted_star -- "$BIN" export quoted.rete --format nq \
      --quoted-triple-syntax rdf-star
    run export_quoted_trig -- "$BIN" export quoted.rete --format trig
    run export_quoted_trig_star -- "$BIN" export quoted.rete --format trig \
      --quoted-triple-syntax rdf-star

    cp export_raw.out   raw.nq
    cp export_clean.out clean.nq
    cp export_unrep.out unrepairable-sanitized.nq
    cp export_named.out named-export.nq
    cp export_quoted.out      quoted-export.nq
    cp export_quoted_star.out quoted-star.nq
    cp export_quoted_trig.out quoted-export.trig
    cp export_quoted_trig_star.out quoted-star.trig

    # rete reading its OWN TriG back, in each surface. The store on the other
    # side of this suite proves the dumps are valid RDF 1.2 / RDF-star; these
    # two prove rete can re-ingest what it wrote, which is a different claim and
    # was false until `rete build --quoted-triple-syntax` existed.
    run build_trig_12 -- "$BIN" build quoted-export.trig -o trig12.rete \
      --format trig --quoted-triple-syntax rdf12
    run build_trig_star -- "$BIN" build quoted-star.trig -o trigstar.rete \
      --format trig --quoted-triple-syntax rdf-star
    run export_trig_12   -- "$BIN" export trig12.rete   --format nq
    run export_trig_star -- "$BIN" export trigstar.rete --format nq
    cp export_trig_12.out   trig12-back.nq
    cp export_trig_star.out trigstar-back.nq

    # The default input surface on an RDF 1.2 dump: a REFUSAL, and one that
    # names the flag. This is the whole reason the input default is `rdf-star`
    # while the output default is `rdf12` — the opposite mistake would have
    # parsed, quietly, into a different graph.
    run build_trig_wrong -- "$BIN" build quoted-export.trig -o wrong.rete \
      --format trig

    # And the ambiguity itself, in rete's own terms: the RDF-star TriG read as
    # RDF 1.2 parses fine and yields a DIFFERENT graph (reifiers, blank nodes,
    # `rdf:reifies`). Asserted rather than assumed, because "both values work"
    # would also be true of a flag that did nothing.
    run build_trig_reified -- "$BIN" build quoted-star.trig -o reified.rete \
      --format trig --quoted-triple-syntax rdf12
    run export_trig_reified -- "$BIN" export reified.rete --format nq
    cp export_trig_reified.out reified-back.nq
    ;;
  rebuild)
    # The other direction of the cycle docs/interop.md documents: take what
    # Oxigraph dumped and build a .rete from it.
    run build_back  -- "$BIN" build named-back.nq -o named-back.rete
    run export_back -- "$BIN" export named-back.rete --format nq
    cp export_back.out named-back-export.nq

    run build_repaired_back  -- "$BIN" build clean-back.nq -o clean-back.rete
    run export_repaired_back -- "$BIN" export clean-back.rete --format nq
    cp export_repaired_back.out clean-back-export.nq

    # The quoted-triple cycle. What comes back from Oxigraph is RDF 1.2, which
    # rete's N-Quads tokenizer takes as readily as the surface it stores — so
    # the comparison is against the SAME surface on both sides, and the only
    # thing being tested is whether the graph survived.
    run build_quoted_back  -- "$BIN" build quoted-back.nq -o quoted-back.rete
    run export_quoted_back -- "$BIN" export quoted-back.rete --format nq
    cp export_quoted_back.out quoted-back-export.nq

    # The same cycle through TRIG, which is the half that needed a new reader:
    # what Oxigraph dumps as TriG is RDF 1.2 Turtle syntax, and until
    # `--quoted-triple-syntax rdf12` rete could not read it at all.
    run build_qtrig_back  -- "$BIN" build qtrig-back.trig -o qtrig-back.rete \
      --format trig --quoted-triple-syntax rdf12
    run export_qtrig_back -- "$BIN" export qtrig-back.rete --format nq
    cp export_qtrig_back.out qtrig-back-export.nq
    ;;
  *)
    echo "unknown stage: $STAGE (expected 'export' or 'rebuild')" >&2
    exit 2
    ;;
esac
