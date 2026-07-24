#!/usr/bin/env bash
# Scaffold the standard layout for a new dataset under ./data/<name>/.
#
#   bash skills/dataset-download/scripts/scaffold.sh <name>
#
# Creates data/<name>/{raw,scripts}, a README.md skeleton, an empty
# SHA256SUMS.txt, and a download.sh stub. Never overwrites existing files.
set -euo pipefail

NAME="${1:-}"
if [ -z "$NAME" ]; then
  echo "usage: scaffold.sh <name>   (e.g. proteinbase)" >&2
  exit 2
fi
# find repo root (dir containing ./data), falling back to CWD
ROOT="$(pwd)"
while [ "$ROOT" != "/" ] && [ ! -d "$ROOT/data" ] && [ ! -d "$ROOT/.git" ]; do
  ROOT="$(dirname "$ROOT")"
done
DIR="$ROOT/data/$NAME"

mkdir -p "$DIR/raw" "$DIR/scripts"

if [ ! -f "$DIR/README.md" ]; then
  cat > "$DIR/README.md" <<EOF
# ${NAME}

Raw data snapshot from **<SOURCE NAME>**.

- Source page: <URL>
- License: **<LICENSE>**  (attribution: "<ATTRIBUTION STRING>")
- Snapshot: \`<primary file>\` (Last-Modified <DATE>)
- Downloaded: $(date -u +%Y-%m-%d 2>/dev/null || echo "<DATE>")

## Layout

\`\`\`
data/${NAME}/
  README.md
  SHA256SUMS.txt
  raw/            # downloaded bytes, as-is
  scripts/        # download.sh + inspect.py + helpers (committed to git)
\`\`\`

## Dataset shape

<records / columns / embedded-JSON notes — fill from scripts/inspect.py>

## Reproduce

\`\`\`bash
bash data/${NAME}/scripts/download.sh
MSYS_NO_PATHCONV=1 docker run --rm -v "\$PWD:/w" -w //w python:3.12-slim \\
    python data/${NAME}/scripts/inspect_csv.py
\`\`\`

## Next step

Groundwork for a potential \`${NAME}.rete\` — hand off to the rete-from-graph skill.
EOF
  echo "created $DIR/README.md"
fi

[ -f "$DIR/SHA256SUMS.txt" ] || { : > "$DIR/SHA256SUMS.txt"; echo "created $DIR/SHA256SUMS.txt"; }

if [ ! -f "$DIR/scripts/download.sh" ]; then
  cat > "$DIR/scripts/download.sh" <<EOF
#!/usr/bin/env bash
# Reproducible download of the ${NAME} data. Fill in the real URL(s) from the
# source's download page / API / bucket.
set -euo pipefail
RAW_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")/../raw" && pwd)"

# --- edit: one curl per primary file ---
# FILE="<primary-file-name>"
# curl -sSL --fail "<URL>" -o "\${RAW_DIR}/\${FILE}"
# ( cd "\${RAW_DIR}" && sha256sum "\${FILE}" | tee "../SHA256SUMS.txt" )

echo "TODO: fill in download URLs in \$0" >&2
EOF
  chmod +x "$DIR/scripts/download.sh"
  echo "created $DIR/scripts/download.sh (stub — fill in URLs)"
fi

echo
echo "scaffolded: $DIR"
find "$DIR" -type f | sort
