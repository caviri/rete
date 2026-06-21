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
check "build .nt"  "5 triples"          -- $B build "$T/g.nt" -o "$T/g.rete"
check "build .nq"  "quads"              -- $B build "$T/d.nq" -o "$T/d.rete"
# Multiple inputs merge under one dictionary; `-` reads stdin (--format for it).
printf '<http://ex/Z> <http://ex/p> <http://ex/W> .\n' > "$T/extra.nt"
check "build merge" "triples"           -- $B build "$T/g.nt" "$T/extra.nt" -o "$T/merge.rete"
check "build stdin" "triples"           -- bash -c "cat '$T/g.nt' | $B build - --format nt -o '$T/stdin.rete'"
check "merge has both" "Z"              -- $B query "$T/merge.rete" --subject "<http://ex/Z>"

echo "== validate =="
check "validate ok"    "valid: 5"       -- $B validate "$T/g.nt"
check "validate bad"   "rror|parse"     -- bash -c "printf 'garbage <<<\n' > '$T/bad.nt'; $B validate '$T/bad.nt'; true"
check "shacl ok"       '"conforms": true' -- $B shacl "$T/g.rete" --shapes "$T/person-ok.ttl" --format json
check "shacl bad"      "MinCountConstraintComponent" -- bash -c "$B shacl '$T/g.rete' --shapes '$T/person-bad.ttl' --format json; true"

echo "== inspect =="
check "info"       "magic|version|pyramid"        -- $B info "$T/g.rete"
check "stats"      "triples|terms|predicate"      -- $B stats "$T/g.rete"
check "verify ok"  "OK|matches"     -- $B verify "$T/g.rete"
check "graphs"     "g1|g2"                          -- $B graphs "$T/d.rete"

echo "== dataset card =="
$B build "$T/g.nt" -o "$T/gc.rete" --card --title "Smoke" --license "CC0-1.0" >/dev/null 2>&1
check "card build"  "dataset card"                  -- bash -c "$B build '$T/g.nt' -o '$T/gc2.rete' --card 2>&1"
check "card view"   "Dataset Card|class links|signals|starter queries" -- $B card "$T/gc.rete"
check "card json"   '"format_version"'              -- $B card "$T/gc.rete" --json
check "card json queries" '"queries"'               -- $B card "$T/gc.rete" --json
check "card json tier"    '"tier"|"sparql"'         -- $B card "$T/gc.rete" --json
check "card queries" "ov-triples|starter"           -- $B card "$T/gc.rete"

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
check "schema pyramid"  "schema pyramid|level"      -- $B summary "$T/onto.rete"
check "level 0 abstract" "Person"                   -- $B summary "$T/onto.rete" --level 0
check "level leaf"      "Astronomer"                -- $B summary "$T/onto.rete" --level 2
check "level out of range" "out of range|level"     -- bash -c "$B summary '$T/onto.rete' --level 99; true"

check "export"     "Alice"                          -- $B export "$T/g.rete"
check "export ttl" "<http://ex/Alice>"              -- $B export "$T/g.rete" --format ttl
check "export jsonld" '"@id": "http://ex/Alice"'    -- $B export "$T/g.rete" --format jsonld

echo "== query =="
check "query pred" "Bob|Alice"   -- $B query "$T/g.rete" --predicate "<http://ex/knows>"
check "why"        "index: POS|dictionary" -- $B why "$T/g.rete" --predicate "<http://ex/knows>"
check "why json"   '"index_permutation": "POS"' -- $B why "$T/g.rete" --predicate "<http://ex/knows>" --json
check "bgp"        "Alice|Bob"   -- $B bgp "$T/g.rete" "?x <http://ex/knows> ?y"
check "sparql"     "Bob"         -- $B sparql "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "sparql json" '"bindings"' -- $B sparql "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { ?x e:knows ?y }" --json
check "ask"        "true|boolean" -- $B sparql "$T/g.rete" "PREFIX e: <http://ex/> ASK { ?x e:knows ?y }"
check "cost"       "lazy query open|summary overview" -- $B cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "cost json"  '"current_engine_access": "lazy-tiles"' -- $B cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }" --json
check "cost lazy open" '"lazy_query_open"' -- $B cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }" --json
check "cost summary answer" '"kind": "predicate_count"' -- $B cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s e:knows ?o }" --json
check "cost explain" '"planned_access": "summary-only"' -- $B cost "$T/g.rete" "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s e:knows ?o }" --json --explain
check "progressive count" '"reads_index": false' -- $B progressive "$T/g.rete" "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s e:knows ?o }" --json
check "progressive total" '"query_shape": "triple_count"' -- $B progressive "$T/g.rete" "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }" --json
check "progressive predicate totals" '"query_shape": "predicate_totals"' -- $B progressive "$T/g.rete" "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p" --json
check "progressive predicate list" '"query_shape": "predicate_list"' -- $B progressive "$T/g.rete" "SELECT DISTINCT ?p WHERE { ?s ?p ?o }" --json
check "progressive predicate distinct count" '"query_shape": "predicate_distinct_count"' -- $B progressive "$T/g.rete" "SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }" --json
check "progressive any ask" '"query_shape": "triple_exists"' -- $B progressive "$T/g.rete" "ASK { ?s ?p ?o }" --json

echo "== cypher (translated to SPARQL) =="
# Build the example dependency graph and run Cypher-subset queries against it.
$B build "$ROOT/examples/deps.nt" -o "$T/deps.rete" >/dev/null
# Labeled node match: the one Application node.
check "cypher label" "http://ex/app" -- $B cypher "$T/deps.rete" "MATCH (a:Application) RETURN a"
# Variable-length path → SPARQL `dependsOn+`: what reaches the vulnerable log4x?
check "cypher varlen" "http://ex/web" -- $B cypher "$T/deps.rete" "MATCH (a)-[:dependsOn*]->(b) WHERE b = <http://ex/log4x> RETURN a"
# JSON output path.
check "cypher json" '"bindings"' -- $B cypher "$T/deps.rete" "MATCH (a:Application) RETURN a" --json
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
check "federate merge alice" "Alice" -- $B federate "$T/sa.rete" "$T/sb.rete" --query "$FEDQ"
check "federate merge bob"   "Bob"   -- $B federate "$T/sa.rete" "$T/sb.rete" --query "$FEDQ"
# Dedup + count: exactly 3 merged solutions (Alice, Bob, Shared).
check "federate dedup" "3 merged result" -- bash -c "$B federate '$T/sa.rete' '$T/sb.rete' --query '$FEDQ' 2>&1"
# Routing prunes the predicate-disjoint shard sc (uses ex/label, not ex/cites).
check "federate routing" "1 pruned" -- bash -c "$B federate '$T/sa.rete' '$T/sc.rete' --query '$FEDQ' 2>&1"
# --no-route disables pruning: sc is queried (contributing 0 rows), 0 pruned.
check "federate no-route" "0 pruned" -- bash -c "$B federate '$T/sa.rete' '$T/sc.rete' --query '$FEDQ' --no-route 2>&1"
# ASK federation is a logical OR across shards.
check "federate ask" "true|boolean" -- $B federate "$T/sa.rete" "$T/sb.rete" --query "ASK { ?x <http://ex/cites> <http://ex/T> }"

echo "== reach (multi-source transitive reachability) =="
# Reverse reach = impact analysis: who (transitively) depends on log4x?
check "reach reverse" "reached-by 4 node" -- $B reach "$T/deps.rete" --predicate "<http://ex/dependsOn>" --seed "<http://ex/log4x>" --reverse --count
# Forward reach in parallel must agree (app reaches its whole dependency closure).
check "reach parallel" "reaches [0-9]+ node" -- $B reach "$T/deps.rete" --predicate "<http://ex/dependsOn>" --seed "<http://ex/app>" --parallel --count

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
check "reason coherent" "coherent|no inconsistencies" -- $B reason "$T/coherent.rete"
check "reason infers"   "inferred [0-9]+ new"          -- $B reason "$T/coherent.rete"
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
check "reason stamp card"  "coherence"        -- $B card "$T/causal-stamped.rete"
check "reason verify-card" "verified"         -- $B reason "$T/causal-stamped.rete" --verify-card
check "reason check gate"  "incoherent"       -- bash -c "$B reason '$T/causal-stamped.rete' --check ; true"

echo "== coarse graphs =="
check "summary"    "round|superedge|knows|community" -- $B summary "$T/g.rete"
check "predicates" "knows"                            -- $B predicates "$T/g.rete"
check "schema"     "Person"                           -- $B schema "$T/g.rete"

echo "== communities (per-community membership + literal text) =="
# The papers example has 3 thematic clusters → Louvain finds multiple
# communities, each carrying literal text (the LDA corpus).
$B build "$ROOT/examples/papers.nt" -o "$T/papers.rete" >/dev/null
check "communities human" "community [0-9]+: [0-9]+ members" -- $B communities "$T/papers.rete"
check "communities json"  '"text"'                          -- $B communities "$T/papers.rete" --json
# JSON is well-formed and has the documented shape (community/size/members/text).
check "communities shape" "shape ok" -- bash -c "$B communities '$T/papers.rete' --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert isinstance(d,list) and len(d)>=2; r=d[0]; assert {\"community\",\"size\",\"members\",\"text\"}<=set(r); assert isinstance(r[\"members\"],list) and isinstance(r[\"text\"],list); print(\"shape ok\")'"
# Structural topic profile (no ML): each community gets top words/classes/predicates.
check "communities profile" "topic words" -- $B communities "$T/papers.rete" --profile
check "communities profile json" '"profile"' -- $B communities "$T/papers.rete" --json --profile
# Multi-criteria: partition by a single relation gives a criterion-specific split.
$B build "$ROOT/examples/researchers.nt" -o "$T/res.rete" >/dev/null
check "communities by predicate" "community [0-9]+:" -- $B communities "$T/res.rete" --predicate "<http://ex/coauthor>"

echo "== HTTP range path =="
$B build "$T/g.nt" -o "$T/web.rete" >/dev/null
( cd "$T" && python3 "$ROOT/scripts/range_server.py" 8099 . >/dev/null 2>&1 & echo $! > "$T/srv.pid" )
sleep 1
check "card-url"    "Dataset Card|index NOT fetched" -- $B card-url "http://127.0.0.1:8099/gc.rete"
check "card-url json" '"format_version"|index NOT fetched' -- $B card-url "http://127.0.0.1:8099/gc.rete" --json
check "summary-url" "knows|round" -- $B summary-url "http://127.0.0.1:8099/web.rete"
check "query-url"   "Bob|Alice|result" -- $B query-url "http://127.0.0.1:8099/web.rete" --predicate "<http://ex/knows>"
check "sparql-url"  "Bob|solution" -- $B sparql-url "http://127.0.0.1:8099/web.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "cost-url"    "full query open|range request" -- $B cost "http://127.0.0.1:8099/web.rete" "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
check "shacl-url"   "MinCountConstraintComponent" -- bash -c "$B shacl-url 'http://127.0.0.1:8099/g.rete' --shapes '$T/person-bad.ttl' --format json; true"
check "why-url"     "index_permutation|POS|tile" -- $B why-url "http://127.0.0.1:8099/web.rete" --predicate "<http://ex/knows>" --json
kill "$(cat "$T/srv.pid")" 2>/dev/null

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

echo
if [ "$fails" -eq 0 ]; then echo "SMOKE OK — all CLI commands behaved"; else echo "SMOKE FAILED: $fails check(s)"; fi
rm -rf "$T"
exit "$fails"
