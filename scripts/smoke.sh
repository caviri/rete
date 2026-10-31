#!/usr/bin/env bash
# CLI acceptance smoke test: exercise every `rete` subcommand end-to-end through
# the compiled binary and assert on its output. Catches integration regressions
# (clap wiring, output formats, the HTTP range path) that the library unit/
# integration tests don't cover. Run in the dev container: scripts/smoke.sh
set -uo pipefail

# Repo root, independent of the current directory or container mount path.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

cargo build --release -q -p rete-cli
B="${CARGO_TARGET_DIR:-target}/release/rete"
T=$(mktemp -d)
fails=0

# check NAME EXPECTED_REGEX -- COMMAND...
check() {
  local name="$1" pat="$2"; shift 2; [ "$1" = "--" ] && shift
  local out; out="$("$@" 2>&1)"
  if echo "$out" | grep -qE "$pat"; then
    echo "  ok   $name"
  else
    echo "  FAIL $name"
    echo "       expected /$pat/, got:"; echo "$out" | sed 's/^/         /' | head -5
    fails=$((fails + 1))
  fi
}

# check_code NAME EXPECTED_CODE EXPECTED_REGEX -- COMMAND...
check_code() {
  local name="$1" expected="$2" pat="$3"; shift 3; [ "$1" = "--" ] && shift
  local out code; out="$("$@" 2>&1)"; code=$?
  if [ "$code" -eq "$expected" ] && echo "$out" | grep -qE "$pat"; then
    echo "  ok   $name (exit $code)"
  else
    echo "  FAIL $name"
    echo "       expected exit $expected and /$pat/, got exit $code:"
    echo "$out" | sed 's/^/         /' | head -5
    fails=$((fails + 1))
  fi
}

# --- fixtures -------------------------------------------------------------
cat > "$T/g.nt" <<EOF
<http://ex/Alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/Bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/Alice> <http://ex/knows> <http://ex/Bob> .
<http://ex/Bob> <http://ex/knows> <http://ex/Alice> .
<http://ex/Alice> <http://ex/age> "30" .
EOF
cat > "$T/d.nq" <<EOF
<http://ex/Alice> <http://ex/knows> <http://ex/Bob> <http://ex/g1> .
<http://ex/Bob> <http://ex/age> "25" <http://ex/g2> .
EOF
cat > "$T/person-ok.ttl" <<EOF
@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [
    sh:path ex:knows ;
    sh:minCount 1
  ] .
EOF
cat > "$T/person-bad.ttl" <<EOF
@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [
    sh:path ex:age ;
    sh:minCount 1
  ] .
EOF

echo "== build =="
check "build .nt"  "5 triples"          -- "$B" build "$T/g.nt" -o "$T/g.rete"
check "build .nq"  "quads"              -- "$B" build "$T/d.nq" -o "$T/d.rete"
# Multiple inputs merge under one dictionary; `-` reads stdin (--format for it).
printf '<http://ex/Z> <http://ex/p> <http://ex/W> .\n' > "$T/extra.nt"
check "build merge" "triples"           -- "$B" build "$T/g.nt" "$T/extra.nt" -o "$T/merge.rete"
check "build stdin" "triples"           -- bash -c "cat '$T/g.nt' | $B build - --format nt -o '$T/stdin.rete'"
check "merge has both" "Z"              -- "$B" query "$T/merge.rete" --subject "<http://ex/Z>"
# Writer knobs change the bytes, never the answers; the memory-bounded build
# writes the in-memory build's bytes under every one, promotion included.
check "build knobs" "5 triples"         -- "$B" build "$T/g.nt" -o "$T/knobs.rete" --tile-kib 16 \
  --dict-chunk-kib 8 --zstd-level 19 --frequent-objects 4
check "knobs verify" "OK|matches"       -- "$B" verify "$T/knobs.rete"
check "knobs answer" "ex/Bob"           -- "$B" query "$T/knobs.rete" --object "<http://ex/Person>"
check "knobs ext = ram" "^same$"        -- bash -c "$B build '$T/g.nt' -o '$T/knobs-ram.rete' --no-pyramid --frequent-objects 4 >/dev/null 2>&1; $B build '$T/g.nt' -o '$T/knobs-ext.rete' --memory-budget-mb 64 --frequent-objects 4 >/dev/null 2>&1; cmp -s '$T/knobs-ram.rete' '$T/knobs-ext.rete' && echo same || echo differ"
check_code "tile-kib range" 2 "4..=4096|4096"  -- "$B" build "$T/g.nt" -o "$T/x.rete" --tile-kib 2

echo "== full-text index =="
cat > "$T/text.nq" <<EOF
<http://ex/w1> <http://ex/title> "Glucose metabolism" .
<http://ex/w1> <http://ex/desc> "The entity has been modified." .
<http://ex/w1/prov> <http://ex/desc> "The entity has been created." <http://ex/w1/g> .
EOF
check "text-index build"      "quads"                -- "$B" build "$T/text.nq" -o "$T/text.rete" --text-index --no-pyramid
# The memory-bounded build takes --text-index and writes the same bytes.
check "text-index ext build"  "extbuild: text index" -- "$B" build "$T/text.nq" -o "$T/text-ext.rete" --text-index --memory-budget-mb 64
check "text-index ext = ram"  "^same$"               -- bash -c "cmp '$T/text.rete' '$T/text-ext.rete' && echo same"
check "search --contains"     "<http://ex/w1>"       -- "$B" search "$T/text-ext.rete" --contains glucose
check "scope: named graphs"   "<http://ex/w1/prov>"  -- bash -c "$B build '$T/text.nq' -o '$T/text-all.rete' --text-index --text-index-graphs all --memory-budget-mb 64 >/dev/null 2>&1 && $B search '$T/text-all.rete' --contains created"
check "scope: exclude"        "covers: .*all but <http://ex/desc>" -- bash -c "$B build '$T/text.nq' -o '$T/text-ex.rete' --text-index --text-index-exclude http://ex/desc >/dev/null 2>&1 && $B search '$T/text-ex.rete' --contains modified 2>&1"
check "scope: stats kind 11"  "some predicates only" -- "$B" stats "$T/text-ex.rete"
check "scope: repyramid"      "<http://ex/w1/prov>"  -- bash -c "$B repyramid '$T/text.rete' -o '$T/text-rp.rete' --text-index --text-index-graphs all >/dev/null 2>&1 && $B search '$T/text-rp.rete' --contains created"
check_code "scope: needs --text-index" 2 "required" -- "$B" build "$T/text.nq" -o "$T/x.rete" --text-index-exclude http://ex/desc

echo "== validate =="
check "validate ok"    "valid: 5"       -- "$B" validate "$T/g.nt"
check "validate bad"   "rror|parse"     -- bash -c "printf 'garbage <<<\n' > '$T/bad.nt'; $B validate '$T/bad.nt'; true"
check "shacl ok"       '"conforms": true' -- "$B" shacl "$T/g.rete" --shapes "$T/person-ok.ttl" --format json
check "shacl bad"      "MinCountConstraintComponent" -- bash -c "$B shacl '$T/g.rete' --shapes '$T/person-bad.ttl' --format json; true"

echo "== release support =="
if $B generate --output "$T/release-docs"; then
  echo "  ok   generate release docs"
else
  echo "  FAIL generate release docs"
  fails=$((fails + 1))
fi
for artifact in rete.bash _rete rete.fish rete.ps1 rete.1; do
  if [ -s "$T/release-docs/$artifact" ]; then
    echo "  ok   generated $artifact"
  else
    echo "  FAIL generated $artifact"
    fails=$((fails + 1))
  fi
done

echo "== inspect =="
check "info"       "magic|version|pyramid"        -- "$B" info "$T/g.rete"
check "stats"      "triples|terms|predicate"      -- "$B" stats "$T/g.rete"
check "verify ok"  "OK|matches"     -- "$B" verify "$T/g.rete"
check "graphs"     "g1|g2"                          -- "$B" graphs "$T/d.rete"
# The header counts every statement; stats splits it (d.nq: 2 quads, 2 graphs).
check "stats split"  "named-graph quads +: 2"        -- "$B" stats "$T/d.rete"
check "stats dg"     "default-graph triples : 0"     -- "$B" stats "$T/d.rete"
# The local open is memory-aware; RETE_OPEN_DEBUG says which way it went and why.
check "open eager"   "threshold=[0-9]+ [(].+[)] -> EAGER" -- env RETE_OPEN_DEBUG=1 "$B" stats "$T/g.rete"
check "open lazy"    "RETE_LOCAL_LAZY_ABOVE_MB=0[)] -> LAZY" -- env RETE_OPEN_DEBUG=1 RETE_LOCAL_LAZY_ABOVE_MB=0 "$B" stats "$T/g.rete"

echo "== label index + schema sections (kinds 8, 9) =="
cat > "$T/lab.nt" <<EOF
<http://ex/w1> <http://purl.org/dc/terms/title> "Acta Universitatis Agriculturae" .
<http://ex/w1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Journal> .
<http://ex/w1> <http://ex/publisher> <http://ex/p1> .
<http://ex/p1> <http://xmlns.com/foaf/0.1/familyName> "Svobodová" .
<http://ex/p1> <http://xmlns.com/foaf/0.1/givenName> "Jana" .
EOF
check "build --label-index"    "5 triples"         -- "$B" build "$T/lab.nt" -o "$T/lab.rete" --no-pyramid --label-index
check "search label section"   "Acta Universitatis" -- "$B" search "$T/lab.rete" "acta u"
check "search composed name"   "Svobodová, Jana"   -- "$B" search "$T/lab.rete" "svobodová"
check "stats label index"      "label index: 2 labels" -- "$B" stats "$T/lab.rete"
check "stats schema, no pyramid" "planner stats"   -- "$B" stats "$T/lab.rete"
check "summary schema section" "SCHEMA section"    -- "$B" summary "$T/lab.rete"
check "verify sections"        "OK|matches"        -- "$B" verify "$T/lab.rete"
check "no label index"         "no label index"    -- bash -c "$B search '$T/g.rete' ali 2>&1"
check "memory-bounded labels"  "triples|quads"     -- "$B" build "$T/lab.nt" -o "$T/labx.rete" --memory-budget-mb 64 --label-index
check "external = in-memory"   "^same$"            -- bash -c "cmp -s '$T/lab.rete' '$T/labx.rete' && echo same || echo differ"
check "card label signal"      '"entries": ?2' -- bash -c "$B build '$T/lab.nt' -o '$T/labc.rete' --label-index --card >/dev/null 2>&1; $B card '$T/labc.rete' --json"

# The quad index (`--named-graphs quad`): same graphs, same answers.
check "build quad index" "quads"                    -- "$B" build "$T/d.nq" -o "$T/dq.rete" --named-graphs quad
check "quad graphs"   "g1|g2"                       -- "$B" graphs "$T/dq.rete"
check "quad GRAPH <g>" "Bob"                        -- "$B" sparql "$T/dq.rete" 'SELECT ?o WHERE { GRAPH <http://ex/g1> { ?s <http://ex/knows> ?o } }'
check "quad verify"   "OK|matches"                  -- "$B" verify "$T/dq.rete"
check "quad export"   "<http://ex/g2>"              -- "$B" export "$T/dq.rete"
check "quad external = in-memory" "^same$"     -- bash -c "$B build '$T/d.nq' -o '$T/dqr.rete' --no-pyramid --named-graphs quad --text-index --text-index-graphs all --label-index >/dev/null 2>&1; $B build '$T/d.nq' -o '$T/dqx.rete' --memory-budget-mb 64 --named-graphs quad --text-index --text-index-graphs all --label-index >/dev/null 2>&1; cmp -s '$T/dqr.rete' '$T/dqx.rete' && echo same || echo differ"

echo "== dataset card =="
$B build "$T/g.nt" -o "$T/gc.rete" --card --title "Smoke" --license "CC0-1.0" >/dev/null 2>&1
check "card build"  "dataset card"                  -- bash -c "$B build '$T/g.nt' -o '$T/gc2.rete' --card 2>&1"
check "card view"   "Dataset Card|class links|signals|starter queries" -- "$B" card "$T/gc.rete"
check "card json"   '"format_version"'              -- "$B" card "$T/gc.rete" --json
check "card json queries" '"queries"'               -- "$B" card "$T/gc.rete" --json
check "card json tier"    '"tier"|"sparql"'         -- "$B" card "$T/gc.rete" --json
check "card queries" "ov-triples|starter"           -- "$B" card "$T/gc.rete"
# The audit, static and measured. `--measure` runs the shipped queries through
# the same measurement the build used, so the figures it prints must agree with
# the ones already inside the file ("= build record").
check "card-audit"  "ov-triples"                    -- "$B" card-audit "$T/gc.rete"
check "card-audit measure" "= build record"         -- "$B" card-audit "$T/gc.rete" --measure
check "card-audit transport" "measured over: local file" -- "$B" card-audit "$T/gc.rete" --measure
check "card-audit only"    "1 query run"            -- "$B" card-audit "$T/gc.rete" --measure --only ov-triples
# Writing the costs back keeps the file's identity: same content hash, still
# verifies, and the costs read back from the CARD tier.
cp "$T/gc.rete" "$T/gcw.rete"
$B build "$T/g.nt" -o "$T/gcw.rete" --card --no-card-costs >/dev/null 2>&1
check "card-audit write" "content hash unchanged" -- bash -c "$B card-audit '$T/gcw.rete' --measure --write-costs --allow-empty 2>&1"
check "card-audit write verify" "OK|matches"      -- "$B" verify "$T/gcw.rete"
check "card-audit write costs"  "query costs"     -- "$B" card "$T/gcw.rete"
# The curated example_queries are audited too, apart from the generated ones.
printf '%s\n' '{"title":"Smoke","example_queries":["SELECT ?s WHERE { ?s ?p ?o } LIMIT 1","SELECT ?s WHERE {"]}' > "$T/gcx.json"
$B build "$T/g.nt" -o "$T/gcx.rete" --card-file "$T/gcx.json" >/dev/null 2>&1
check "card-audit curated" "curated example_queries \(2\)" -- "$B" card-audit "$T/gcx.rete"
check "card-audit invalid" "invalid +example-2"   -- "$B" card-audit "$T/gcx.rete"

# The build record: how the file was made — flags, engine, inputs by NAME (never
# a path) with their BLAKE3, counts, wall time per phase, peak RAM.
check "record engine"   "engine +: rete-cli"        -- "$B" card "$T/gcx.rete"
check "record command"  "command +: rete build --permutations 6 --named-graphs per-graph" -- "$B" card "$T/gcx.rete"
check "record inputs"   "inputs +: g\.nt \(nt\), [0-9,]+ B, blake3 [0-9a-f]{16}" -- "$B" card "$T/gcx.rete"
check "record counts"   "statements +: 5 read, 5 unique"       -- "$B" card "$T/gcx.rete"
check "record run"      "wall time +: .* on [0-9]+ CPU"        -- "$B" card "$T/gcx.rete"
check "record card file" '"name": ?"gcx\.json"'               -- "$B" card "$T/gcx.rete" --json
check "record no path"  "^clean$" -- bash -c "$B card '$T/gcx.rete' --json | grep -q '$T' && echo leaked || echo clean"
check "record prov"     '"prov:startedAtTime"'                -- "$B" card "$T/gcx.rete" --format jsonld
$B build "$T/g.nt" -o "$T/gcxe.rete" --card-file "$T/gcx.json" --memory-budget-mb 64 >/dev/null 2>&1
check "record spill"    "spill +: 1 chunk"                     -- "$B" card "$T/gcxe.rete"
# RETE_BUILD_RECORD=reproducible drops the volatile facts: two builds, one file.
for n in 1 2; do
  RETE_BUILD_RECORD=reproducible SOURCE_DATE_EPOCH=1785801600 \
    "$B" build "$T/g.nt" -o "$T/repro$n.rete" --card-file "$T/gcx.json" >/dev/null 2>&1
done
check "record reproducible" "^same$" -- bash -c "cmp -s '$T/repro1.rete' '$T/repro2.rete' && echo same || echo differ"
check "card-audit curated json" '"example_findings"' -- "$B" card-audit "$T/gcx.rete" --json
check "card-audit curated run" "1 curated query run" -- "$B" card-audit "$T/gcx.rete" --measure --only example-1
# A card-only rewrite: the card and the build record change, nothing else.
check "recard" "starter quer"                     -- "$B" recard "$T/gcx.rete" -o "$T/gcr.rete" --keep-curated --verify
check "recard verify" "OK|matches"                -- "$B" verify "$T/gcr.rete"
check "recard keeps curated" '"title": ?"Smoke"' -- "$B" card "$T/gcr.rete" --json
check "recard record" "re-carded"                 -- "$B" card "$T/gcr.rete"
check_code "recard no overwrite" 1 "already exists" -- "$B" recard "$T/gcx.rete" -o "$T/gcr.rete" --keep-curated
check_code "recard keeps prose" 1 "curated fields" -- "$B" recard "$T/gcx.rete" -o "$T/gcr2.rete"
# The memory-bounded build writes the full card — starter queries included —
# after the data, the card `rete recard` derives from the same file.
check "bounded full card" "starter queries \([1-9]" -- "$B" card "$T/gcxe.rete"
check "bounded card phases" "content_hash" -- "$B" card "$T/gcxe.rete"
$B recard "$T/gcxe.rete" -o "$T/gcxr.rete" --keep-curated >/dev/null 2>&1
check "bounded card = recard" "^same$" -- bash -c "cmp -s <(dd if='$T/gcxe.rete' bs=1 skip=8 count=16 2>/dev/null) <(dd if='$T/gcxr.rete' bs=1 skip=8 count=16 2>/dev/null) && echo same || echo differ"
# In place: the card appended after the data, the header rewritten last.
cp "$T/gcx.rete" "$T/gci.rete"
check "recard in place" "dead byte" -- "$B" recard "$T/gci.rete" --in-place --keep-curated --verify
check "recard in place verify" "OK|matches" -- "$B" verify "$T/gci.rete"
check "recard in place record" '"in_place": ?true' -- "$B" card "$T/gci.rete" --json
check_code "recard -o or --in-place" 2 "in-place|output" -- "$B" recard "$T/gci.rete" --in-place -o "$T/gci2.rete" --keep-curated
# `rete merge` with a card writes the full card too — the one `rete recard`
# derives from the merged file — and its build record names the shards.
$B build "$T/extra.nt" -o "$T/gmx.rete" >/dev/null 2>&1
check "merge full card" "card .* after the data" -- "$B" merge "$T/gcx.rete" "$T/gmx.rete" -o "$T/gm.rete" --card-file "$T/gcx.json" --memory-budget-mb 64
check "merge card queries" "starter queries \([1-9]" -- "$B" card "$T/gm.rete"
check "merge record inputs" "inputs +: gcx\.rete \(rete\), [0-9,]+ B, blake3 [0-9a-f]{16}" -- "$B" card "$T/gm.rete"
check "merge verify" "OK|matches" -- "$B" verify "$T/gm.rete"
$B recard "$T/gm.rete" -o "$T/gmr.rete" --keep-curated >/dev/null 2>&1
check "merge card = recard" "^same$" -- bash -c "cmp -s <(dd if='$T/gm.rete' bs=1 skip=8 count=16 2>/dev/null) <(dd if='$T/gmr.rete' bs=1 skip=8 count=16 2>/dev/null) && echo same || echo differ"

echo "== progress (--progress) =="
# On stderr only: stdout keeps its one result line, the file is the same bytes,
# and `off` (like the default off a terminal) prints nothing.
check "progress plain"        "^progress: build done in" -- bash -c "$B build '$T/g.nt' -o '$T/pp.rete' --progress plain 2>&1 >/dev/null"
check "progress json bounded" '"event":"done"'           -- bash -c "$B build '$T/g.nt' -o '$T/pj.rete' --memory-budget-mb 64 --progress json 2>&1 >/dev/null | tail -1"
check "progress stdout clean" "^wrote [^ ]+: 5 triples"  -- bash -c "$B build '$T/g.nt' -o '$T/ps.rete' --progress json 2>/dev/null"
check "progress same bytes"   "^same$"                   -- bash -c "$B build '$T/g.nt' -o '$T/p0.rete' --progress off >/dev/null 2>&1; $B build '$T/g.nt' -o '$T/p1.rete' --progress json >/dev/null 2>&1; cmp -s '$T/p0.rete' '$T/p1.rete' && echo same || echo differ"
check "progress env merge"    "^progress: merge "        -- bash -c "RETE_PROGRESS=plain $B merge '$T/gcx.rete' '$T/gmx.rete' -o '$T/pm.rete' --memory-budget-mb 64 2>&1 >/dev/null"
check "progress recard"       '"event":"start"'          -- bash -c "$B recard '$T/gcx.rete' -o '$T/pr.rete' --keep-curated --progress json 2>&1 >/dev/null"
check "progress off"          "^0$"                      -- bash -c "$B build '$T/g.nt' -o '$T/po.rete' --progress off 2>&1 >/dev/null | grep -cE '^(progress:|[{])'"
check_code "progress bad mode" 2 "loud"                  -- "$B" build "$T/g.nt" -o "$T/x.rete" --progress loud

echo "== schema pyramid (semantic zoom) =="
# A tiny subClassOf hierarchy: Astronomer ⊑ Scientist ⊑ Person, with instances.
cat > "$T/onto.nt" <<'ONTO'
<http://ex/Astronomer> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Scientist> .
<http://ex/Scientist> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Person> .
<http://ex/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Astronomer> .
<http://ex/b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Astronomer> .
<http://ex/c> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/a> <http://ex/knows> <http://ex/b> .
<http://ex/b> <http://ex/knows> <http://ex/c> .
ONTO
$B build "$T/onto.nt" -o "$T/onto.rete" >/dev/null 2>&1
check "schema pyramid"  "schema pyramid|level"      -- "$B" summary "$T/onto.rete"
check "level 0 abstract" "Person"                   -- "$B" summary "$T/onto.rete" --level 0
check "level leaf"      "Astronomer"                -- "$B" summary "$T/onto.rete" --level 2
check "level out of range" "out of range|level"     -- bash -c "$B summary '$T/onto.rete' --level 99; true"

check "export"     "Alice"                          -- "$B" export "$T/g.rete"
check "export in-memory" "Alice"                    -- "$B" export "$T/g.rete" --in-memory
check "export ttl" "<http://ex/Alice>"              -- "$B" export "$T/g.rete" --format ttl
check "export jsonld" '"@id": "http://ex/Alice"'    -- "$B" export "$T/g.rete" --format jsonld

echo "== query =="
check "query pred" "Bob|Alice"   -- "$B" query "$T/g.rete" --predicate "<http://ex/knows>"
check "why"        "index: POS|dictionary" -- "$B" why "$T/g.rete" --predicate "<http://ex/knows>"
check "why json"   '"index_permutation": "POS"' -- "$B" why "$T/g.rete" --predicate "<http://ex/knows>" --json
check "bgp"        "Alice|Bob"   -- "$B" bgp "$T/g.rete" "?x <http://ex/knows> ?y"
check "sparql"     "Bob"         -- "$B" sparql "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "sparql json" '"bindings"' -- "$B" sparql "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { ?x e:knows ?y }" --json
check "ask"        "true|boolean" -- "$B" sparql "$T/g.rete" "PREFIX e: <http://ex/> ASK { ?x e:knows ?y }"
# A query too long for the command line goes in a file, or on stdin (`-`).
printf '%s' "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }" > "$T/q.rq"
check "sparql query-file" "Bob" -- "$B" sparql "$T/g.rete" --query-file "$T/q.rq"
# shellcheck disable=SC2016  # $0/$1/$2 belong to the inner sh -c, not this shell
check "sparql query-file stdin" "Bob" -- sh -c '"$0" sparql "$1" --query-file - < "$2"' "$B" "$T/g.rete" "$T/q.rq"
check_code "query and query-file" 2 "cannot be used with" -- "$B" sparql "$T/g.rete" "ASK {}" --query-file "$T/q.rq"
# A FILTER type error (CONTAINS on an IRI) is silently false — rows stay
# spec-correct, and the warning goes to stderr, never into stdout or the rows.
WQ='PREFIX e: <http://ex/> SELECT ?y WHERE { ?x e:knows ?y FILTER(CONTAINS(?y, "Bob")) }'
check "sparql type-error warning" "warning: CONTAINS received an IRI as argument 1" -- "$B" sparql "$T/g.rete" "$WQ"
# shellcheck disable=SC2016  # $0/$1/$2 belong to the inner sh -c, not this shell
check "warning off stdout" "^0 lines$" -- sh -c 'printf "%s lines" "$("$0" sparql "$1" "$2" 2>/dev/null | grep -c warning)"' "$B" "$T/g.rete" "$WQ"
check "sparql json warnings" '"argKind": "iri"' -- "$B" sparql "$T/g.rete" "$WQ" --json
check "cost"       "lazy query open|summary overview" -- "$B" cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "cost json"  '"current_engine_access": "lazy-tiles"' -- "$B" cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }" --json
check "cost lazy open" '"lazy_query_open"' -- "$B" cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }" --json
check "cost summary answer" '"kind": "predicate_count"' -- "$B" cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s e:knows ?o }" --json
check "cost explain" '"planned_access": "summary-only"' -- "$B" cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s e:knows ?o }" --json --explain
check "progressive count" '"reads_index": false' -- "$B" progressive "$T/g.rete" "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s e:knows ?o }" --json
check "progressive total" '"query_shape": "triple_count"' -- "$B" progressive "$T/g.rete" "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }" --json
check "progressive predicate totals" '"query_shape": "predicate_totals"' -- "$B" progressive "$T/g.rete" "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p" --json
check "progressive predicate list" '"query_shape": "predicate_list"' -- "$B" progressive "$T/g.rete" "SELECT DISTINCT ?p WHERE { ?s ?p ?o }" --json
check "progressive predicate distinct count" '"query_shape": "predicate_distinct_count"' -- "$B" progressive "$T/g.rete" "SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }" --json
check "progressive any ask" '"query_shape": "triple_exists"' -- "$B" progressive "$T/g.rete" "ASK { ?s ?p ?o }" --json

echo "== cypher (translated to SPARQL) =="
# Build the example dependency graph and run Cypher-subset queries against it.
$B build "$ROOT/examples/deps.nt" -o "$T/deps.rete" >/dev/null
# Labeled node match: the one Application node.
check "cypher label" "http://ex/app" -- "$B" cypher "$T/deps.rete" "MATCH (a:Application) RETURN a"
# Variable-length path → SPARQL `dependsOn+`: what reaches the vulnerable log4x?
check "cypher varlen" "http://ex/web" -- "$B" cypher "$T/deps.rete" "MATCH (a)-[:dependsOn*]->(b) WHERE b = <http://ex/log4x> RETURN a"
# JSON output path.
check "cypher json" '"bindings"' -- "$B" cypher "$T/deps.rete" "MATCH (a:Application) RETURN a" --json
# A query outside the subset must fail cleanly (no panic).
check "cypher reject" "not supported|Cypher error" -- bash -c "$B cypher '$T/deps.rete' 'CREATE (a) RETURN a'; true"

echo "== federate (union across sharded files + routing) =="
# Two shards that each independently yield complete rows for the query: shard A
# has Alice & a shared node citing T; shard B has Bob & the same shared node.
# Federation unions + dedups → {Alice, Bob, shared} (the shared row dedups).
cat > "$T/sa.nt" <<EOF
<http://ex/Alice> <http://ex/cites> <http://ex/T> .
<http://ex/Shared> <http://ex/cites> <http://ex/T> .
EOF
cat > "$T/sb.nt" <<EOF
<http://ex/Bob> <http://ex/cites> <http://ex/T> .
<http://ex/Shared> <http://ex/cites> <http://ex/T> .
EOF
# A third shard uses a DIFFERENT predicate, so a `cites` query must prune it.
cat > "$T/sc.nt" <<EOF
<http://ex/Org> <http://ex/label> "an org" .
EOF
$B build "$T/sa.nt" -o "$T/sa.rete" >/dev/null
$B build "$T/sb.nt" -o "$T/sb.rete" >/dev/null
$B build "$T/sc.nt" -o "$T/sc.rete" >/dev/null
FEDQ='SELECT ?x WHERE { ?x <http://ex/cites> <http://ex/T> }'
# Merged result has all three distinct citing nodes (shared row deduped to one).
check "federate merge alice" "Alice" -- "$B" federate "$T/sa.rete" "$T/sb.rete" --query "$FEDQ"
check "federate merge bob"   "Bob"   -- "$B" federate "$T/sa.rete" "$T/sb.rete" --query "$FEDQ"
# Dedup + count: exactly 3 merged solutions (Alice, Bob, Shared).
check "federate dedup" "3 merged result" -- bash -c "$B federate '$T/sa.rete' '$T/sb.rete' --query '$FEDQ' 2>&1"
# Routing prunes the predicate-disjoint shard sc (uses ex/label, not ex/cites).
check "federate routing" "1 pruned" -- bash -c "$B federate '$T/sa.rete' '$T/sc.rete' --query '$FEDQ' 2>&1"
# --no-route disables pruning: sc is queried (contributing 0 rows), 0 pruned.
check "federate no-route" "0 pruned" -- bash -c "$B federate '$T/sa.rete' '$T/sc.rete' --query '$FEDQ' --no-route 2>&1"
# ASK federation is a logical OR across shards.
check "federate ask" "true|boolean" -- "$B" federate "$T/sa.rete" "$T/sb.rete" --query "ASK { ?x <http://ex/cites> <http://ex/T> }"
# A URL source opens lazily (as sparql-url does) and reports what it fetched.
( cd "$T" && python3 "$ROOT/scripts/range_server.py" 8097 . >/dev/null 2>&1 & echo $! > "$T/srv2.pid" )
sleep 1
check "federate url lazy" "3 merged result" -- bash -c "$B federate 'http://127.0.0.1:8097/sa.rete' '$T/sb.rete' --query '$FEDQ' 2>&1"
check "federate url fetched" "range request" -- bash -c "$B federate 'http://127.0.0.1:8097/sa.rete' '$T/sb.rete' --query '$FEDQ' 2>&1"
kill "$(cat "$T/srv2.pid")" 2>/dev/null

echo "== default query memory budget =="
# Sized from the memory available unless RETE_QUERY_BUDGET_MB (or the flag) says otherwise.
check "budget default" "query_budget\] default [0-9]+ MiB" -- bash -c "RETE_OPEN_DEBUG=1 $B sparql '$T/sa.rete' '$FEDQ' 2>&1"
check "budget env" "default 300 MiB \(RETE_QUERY_BUDGET_MB=300\)" -- bash -c "RETE_OPEN_DEBUG=1 RETE_QUERY_BUDGET_MB=300 $B sparql '$T/sa.rete' '$FEDQ' 2>&1"

echo "== reach (multi-source transitive reachability) =="
# Reverse reach = impact analysis: who (transitively) depends on log4x?
check "reach reverse" "reached-by 4 node" -- "$B" reach "$T/deps.rete" --predicate "<http://ex/dependsOn>" --seed "<http://ex/log4x>" --reverse --count
# Forward reach in parallel must agree (app reaches its whole dependency closure).
check "reach parallel" "reaches [0-9]+ node" -- "$B" reach "$T/deps.rete" --predicate "<http://ex/dependsOn>" --seed "<http://ex/app>" --parallel --count

echo "== reason (OWL RL / RDFS prototype) =="
# A coherent graph: subClassOf chain + transitive property, no contradictions.
cat > "$T/coherent.nt" <<EOF
<http://ex/Disease> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Factor> .
<http://ex/causes> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#TransitiveProperty> .
<http://ex/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Disease> .
<http://ex/a> <http://ex/causes> <http://ex/b> .
<http://ex/b> <http://ex/causes> <http://ex/c> .
EOF
$B build "$T/coherent.nt" -o "$T/coherent.rete" >/dev/null
# Coherent → exit 0, reports no inconsistency, still materializes entailments.
check "reason coherent" "coherent|no inconsistencies" -- "$B" reason "$T/coherent.rete"
check "reason infers"   "inferred [0-9]+ new"          -- "$B" reason "$T/coherent.rete"
# The example causal graph carries a disjoint-class violation → exit non-zero,
# message names the disjointness. `reason` exits non-zero on incoherence, so
# wrap in `; true` to keep the smoke script going.
$B build "$ROOT/examples/causal.nt" -o "$T/causal.rete" >/dev/null
check "reason disjoint" "disjoint" -- bash -c "$B reason '$T/causal.rete' ; true"
# Materialization composes the transitive :causes chain (Smoking → ... → Death ⇒
# Smoking causes Death) — the entailment side of the same causal-coherence demo.
check "reason causal transitive" "ex/Smoking>.*ex/causes>.*ex/Death" -- bash -c "$B reason '$T/causal.rete' --materialize ; true"
# Build-time stamp: --reason embeds the verdict in the card (no abort on
# incoherence); --verify-card re-checks it; --check is the terse CI gate.
$B build "$ROOT/examples/causal.nt" -o "$T/causal-stamped.rete" --reason >/dev/null
check "reason stamp card"  "coherence"        -- "$B" card "$T/causal-stamped.rete"
check "reason verify-card" "verified"         -- "$B" reason "$T/causal-stamped.rete" --verify-card
check "reason check gate"  "incoherent"       -- bash -c "$B reason '$T/causal-stamped.rete' --check ; true"

echo "== coarse graphs =="
check "summary"    "round|superedge|knows|community" -- "$B" summary "$T/g.rete"
check "predicates" "knows"                            -- "$B" predicates "$T/g.rete"
check "schema"     "Person"                           -- "$B" schema "$T/g.rete"

echo "== communities (per-community membership + literal text) =="
# The papers example has 3 thematic clusters → Louvain finds multiple
# communities, each carrying literal text (the LDA corpus).
$B build "$ROOT/examples/papers.nt" -o "$T/papers.rete" >/dev/null
check "communities human" "community [0-9]+: [0-9]+ members" -- "$B" communities "$T/papers.rete"
check "communities json"  '"text"'                          -- "$B" communities "$T/papers.rete" --json
# JSON is well-formed and has the documented shape (community/size/members/text).
check "communities shape" "shape ok" -- bash -c "$B communities '$T/papers.rete' --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d[\"schemaVersion\"]==1; rows=d[\"communities\"]; assert isinstance(rows,list) and len(rows)>=2; r=rows[0]; assert {\"community\",\"size\",\"members\",\"text\"}<=set(r); assert isinstance(r[\"members\"],list) and isinstance(r[\"text\"],list); print(\"shape ok\")'"
# Structural topic profile (no ML): each community gets top words/classes/predicates.
check "communities profile" "topic words" -- "$B" communities "$T/papers.rete" --profile
check "communities profile json" '"profile"' -- "$B" communities "$T/papers.rete" --json --profile
# Multi-criteria: partition by a single relation gives a criterion-specific split.
$B build "$ROOT/examples/researchers.nt" -o "$T/res.rete" >/dev/null
check "communities by predicate" "community [0-9]+:" -- "$B" communities "$T/res.rete" --predicate "<http://ex/coauthor>"

echo "== HTTP range path =="
$B build "$T/g.nt" -o "$T/web.rete" >/dev/null
( cd "$T" && python3 "$ROOT/scripts/range_server.py" 8099 . >/dev/null 2>&1 & echo $! > "$T/srv.pid" )
sleep 1
check "card-url"    "Dataset Card|index NOT fetched" -- "$B" card-url "http://127.0.0.1:8099/gc.rete"
check "card-url json" '"format_version"|index NOT fetched' -- "$B" card-url "http://127.0.0.1:8099/gc.rete" --json
check "summary-url" "knows|round" -- "$B" summary-url "http://127.0.0.1:8099/web.rete"
check "query-url"   "Bob|Alice|result" -- "$B" query-url "http://127.0.0.1:8099/web.rete" --predicate "<http://ex/knows>"
check "sparql-url"  "Bob|solution" -- "$B" sparql-url "http://127.0.0.1:8099/web.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "cost-url"    "full query open|range request" -- "$B" cost "http://127.0.0.1:8099/web.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "shacl-url"   "MinCountConstraintComponent" -- bash -c "$B shacl-url 'http://127.0.0.1:8099/g.rete' --shapes '$T/person-bad.ttl' --format json; true"
check "why-url"     "index_permutation|POS|tile" -- "$B" why-url "http://127.0.0.1:8099/web.rete" --predicate "<http://ex/knows>" --json
# Same answer with the parallel batches off (one range at a time) and on.
check "sparql-url seq" "Bob|solution" -- env RETE_HTTP_CONCURRENCY=1 RETE_HTTP_RETRIES=0 "$B" sparql-url "http://127.0.0.1:8099/web.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "search-url label index" "Svobodová, Jana" -- "$B" search-url "http://127.0.0.1:8099/lab.rete" "svob"
kill "$(cat "$T/srv.pid")" 2>/dev/null
# A host that ignores Range (python's stock http.server answers 200 with the
# whole body): a small file is kept and answers the same; one over the 256 MiB
# cap (sparse, so it costs no disk) is refused by name, unread.
mkdir -p "$T/plain" && cp "$T/web.rete" "$T/plain/web.rete" && truncate -s 300M "$T/plain/big.rete"
( cd "$T/plain" && python3 -m http.server 8098 --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > "$T/plain.pid" )
sleep 1
check "sparql-url range-ignoring host" "Bob|solution" -- "$B" sparql-url "http://127.0.0.1:8098/web.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check_code "sparql-url whole body over cap" 1 "ignored the Range header.*not kept in memory" -- "$B" sparql-url "http://127.0.0.1:8098/big.rete" "SELECT * WHERE { ?s ?p ?o } LIMIT 1"
kill "$(cat "$T/plain.pid")" 2>/dev/null

echo "== error handling (must fail cleanly, not panic) =="
# Flip one byte of the stored content hash (header bytes 8..24): the file still
# parses, but the recomputed hash no longer matches → clean mismatch, no panic.
python3 -c "
import sys
b=bytearray(open('$T/g.rete','rb').read()); b[8]^=0xff; open('$T/bad.rete','wb').write(b)"
check "verify tamper"  "FAILED|mismatch"       -- bash -c "$B verify '$T/bad.rete'; true"
# A truncated file must also be rejected without panicking.
check "verify trunc"   "FAILED|mismatch|Error|malformed" -- bash -c "head -c 80 '$T/g.rete' > '$T/trunc.rete'; $B verify '$T/trunc.rete'; true"
check "missing file"   "rror|not found|No such"  -- bash -c "$B info '$T/nope.rete'; true"

echo "== exit-code contract =="
check_code "runtime malformed RDF" 1 "Error|parse|line" -- "$B" validate "$T/bad.nt"
check_code "Clap usage"            2 "Usage:"           -- "$B" build
check_code "SHACL non-conformance" 3 "conforms.*false|validation failed" -- "$B" shacl "$T/g.rete" --shapes "$T/person-bad.ttl" --format json
check_code "reasoning incoherence" 3 "incoherent|disjoint" -- "$B" reason "$T/causal.rete" --check

echo
if [ "$fails" -eq 0 ]; then echo "SMOKE OK — all CLI commands behaved"; else echo "SMOKE FAILED: $fails check(s)"; fi
rm -rf "$T"
exit "$fails"
