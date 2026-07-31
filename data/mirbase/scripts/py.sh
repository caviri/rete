#!/usr/bin/env bash
# Run one of the miRBase python scripts inside the mirbase-py image.
# Everything runs in Docker (repo convention) — nothing is installed on the host.
#
#   bash data/mirbase/scripts/py.sh fa_to_parquet.py [args...]
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
export MSYS_NO_PATHCONV=1

# Docker Desktop on Windows needs a Windows-style path (D:/pro/rete), not the
# Git Bash one (/d/pro/rete); `pwd -W` gives it where available.
WINREPO="$(cd "$REPO" && { pwd -W 2>/dev/null || pwd; })"

if ! docker image inspect mirbase-py:latest >/dev/null 2>&1; then
  echo "==> building mirbase-py:latest (one time)" >&2
  docker build -q -t mirbase-py:latest "$WINREPO/data/mirbase/scripts" >&2
fi

exec docker run --rm -v "$WINREPO:/w" -w //w mirbase-py:latest \
  python "data/mirbase/scripts/$@"
