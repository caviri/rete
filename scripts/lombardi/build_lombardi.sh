#!/usr/bin/env bash
# Mark Lombardi Networks -> web/lombardi.rete  (harvest is fetch_lombardi.sh)
#
# Three inputs go into one file: the ABox, this project's extension ontology, and
# Tolksdorf's own lombardi.owl -- so the meaning of every arc style travels
# inside the .rete and `--entail` can reason over it.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RETE="$ROOT/.claude/skills/rete-from-graph/scripts/rete"

python "$ROOT/scripts/lombardi/lombardi_to_nt.py"

"$RETE" validate /work/data/lombardi/lombardi.nt

"$RETE" build \
  /work/data/lombardi/lombardi.nt \
  /work/data/lombardi/lombardi-rete.ttl \
  /work/data/lombardi/raw/lombardi.owl \
  -o /work/web/lombardi.rete \
  --pyramid-algo types --text-index --card \
  --title "Mark Lombardi Networks" \
  --license "CC BY-NC-SA 4.0" \
  --source "https://lombardinetworks.net/ (Robert Tolksdorf, FU Berlin)" \
  --description "51 network drawings by Mark Lombardi digitized node by node and arc by arc: 2,934 actors, 4,205 typed arcs whose line style carries the meaning, plus derived cross-drawing overlap."

"$ROOT/.claude/skills/rete-from-graph/scripts/verify_rete.sh" /work/web/lombardi.rete
