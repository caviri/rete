#!/usr/bin/env bash
# Queue guard: wait for the opencitations build to finish, THEN build datacite
# on the freed machine. Polls the opencitations runner log for its exit line.
OC_LOG="/d/pro/rete/data/opencitations/_build_rete.log"
echo "=== datacite QUEUED $(cat /d/pro/rete/data/datacite/_queued_at 2>/dev/null) — waiting for opencitations to finish ==="

while true; do
  if grep -q "build exit code:" "$OC_LOG" 2>/dev/null; then
    rc="$(grep 'build exit code:' "$OC_LOG" | tail -1 | grep -oE '[0-9]+')"
    echo "=== opencitations build finished (exit ${rc}) — starting datacite ==="
    break
  fi
  sleep 120
done

# let the machine settle (opencitations frees its ~6 GB), then build
sleep 45
exec bash /d/pro/rete/data/datacite/scripts/run_build_rete.sh
