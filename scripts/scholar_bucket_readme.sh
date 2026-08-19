#!/usr/bin/env bash
# Generate and publish scholar/README.md — the page someone landing on the
# `scholar/` prefix reads instead of this repository.
#
#   scripts/scholar_bucket_readme.sh              # write + upload
#   scripts/scholar_bucket_readme.sh --local-only # write dev/scholar-nq/README.md and stop
#
# The table is built from the bucket ITSELF (`hf buckets ls`), not from the
# export run's state file: the README must describe what is actually there, and
# a run that failed halfway must not publish a list of files nobody can fetch.
# Quad counts and source URLs come from dev/scholar-nq/done.tsv when it has
# them, and are left blank when it does not.
#
# Options: --bucket NS/NAME  --prefix P  --state FILE  --local-only
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUCKET="${RETE_HF_BUCKET:-katospiegel/rete-public}"
PREFIX="scholar"
STATE="$ROOT/dev/scholar-nq/state.tsv"
OUT="$ROOT/dev/scholar-nq/README.md"
LOCAL_ONLY=0

while [ $# -gt 0 ]; do
  case "$1" in
    --bucket) BUCKET="${2:?}"; shift 2 ;;
    --prefix) PREFIX="${2:?}"; shift 2 ;;
    --state)  STATE="${2:?}"; shift 2 ;;
    --out)    OUT="${2:?}"; shift 2 ;;
    --local-only) LOCAL_ONLY=1; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$(dirname "$OUT")"
[ -f "$STATE" ] || : > "$STATE"

listing="$(hf buckets ls "$BUCKET/$PREFIX" --recursive --json 2>/dev/null)"
[ -n "$listing" ] || { echo "cannot list $BUCKET/$PREFIX" >&2; exit 4; }

# path<TAB>size for every .nq.gz currently in the prefix. `hf buckets ls --json`
# pretty-prints one field per line, so a two-state awk over "path" then "size"
# is enough and pulls in no JSON dependency.
objects="$(printf '%s' "$listing" \
  | awk -F'"' '
      /"path"/ { p = $4 }
      /"size"/ { if (match($3, /[0-9]+/) && p ~ /\.nq\.gz$/) {
                   print p "\t" substr($3, RSTART, RLENGTH); p = "" } }' \
  | LC_ALL=C sort)"

n=$(printf '%s\n' "$objects" | grep -c . )
tot=$(printf '%s\n' "$objects" | awk -F'\t' '{s+=$2} END{printf "%d", s}')

{
cat <<'HDR'
# The scholar constellation, as gzipped N-Quads

Every file here is a **lossless N-Quads dump** of one published `.rete`
knowledge graph — default graph *and* named graphs, RDF-star quoted triples
included — gzipped, one file per dataset. N-Quads is the interchange format
every triple store bulk-loads, so **nothing here needs `rete` to be useful**.

```
scholar/<dataset>/<name>.nq.gz
```

Produced by `rete export <file>.rete --format nq | pigz -6`, from the `.rete`
whose URL is listed against each row. The `.rete` files themselves stay where
they are — served over HTTP range reads at `https://data.graphplaza.com/…`, and
mirrored in this same bucket under `rete/`. Use those if you want to *query* a
graph without loading it; use these if you want it *in your own store*.

HDR

echo "## Loading one"
cat <<'LOAD'

Verified recipes, from the project's interop page. Each takes the gzip
directly — there is no need to decompress first.

### Oxigraph

```sh
oxigraph load --location ./store --file dblp.nq.gz
oxigraph serve --location ./store --bind 0.0.0.0:7878
```

The same subcommands work through the `oxigraph/oxigraph` Docker image.

### GraphDB

Server-side bulk import, which is what you want for anything on this page:

```sh
importrdf preload --force -c repo-config.ttl dblp.nq.gz
```

Into a running instance over REST (what the workbench "Import" does):

```sh
gunzip -c dblp.nq.gz > dblp.nq
curl -X POST "http://localhost:7200/repositories/myrepo/statements" \
  -H "Content-Type: application/n-quads" \
  --data-binary @dblp.nq
```

### Jena / Fuseki

```sh
tdb2.tdbloader --loc ./tdb dblp.nq.gz
fuseki-server --loc ./tdb /ds
```

### Back into rete

`rete build` reads the dump in the form it ships — gzip included, decompressed
while streaming:

```sh
rete build dblp.nq.gz -o dblp.rete
```

If a dataset keeps its data in **named graphs**, note that in SPARQL the default
graph is not the union of the named ones. `rete build --collapse-graphs` folds
them into the default graph; GraphDB and Jena have their own equivalents.

LOAD

echo "## What is here"
echo
echo "$n file(s), $(awk -v b="${tot:-0}" 'BEGIN{printf "%.1f", b/1073741824}') GiB compressed."
echo
echo "| dataset | file | .nq.gz bytes | quads | source \`.rete\` | .rete bytes |"
echo "|---|---|--:|--:|---|--:|"
printf '%s\n' "$objects" | while IFS=$'\t' read -r path size; do
  [ -n "${path:-}" ] || continue
  base="${path##*/}"; name="${base%.nq.gz}"
  rel="${path#"$PREFIX"/}"; ds="${rel%%/*}"
  # state.tsv: status name rete_bytes nqgz_bytes quads url utc — `done` rows only
  q=$(awk -F'\t' -v n="$name" '$1=="done" && $2==n {print $5}' "$STATE" | tail -1)
  ru=$(awk -F'\t' -v n="$name" '$1=="done" && $2==n {print $6}' "$STATE" | tail -1)
  rb=$(awk -F'\t' -v n="$name" '$1=="done" && $2==n {print $3}' "$STATE" | tail -1)
  printf '| %s | `%s` | %s | %s | %s | %s |\n' \
    "$ds" "$rel" "$size" "${q:-—}" "${ru:+[\`.rete\`]($ru)}" "${rb:-—}"
done

cat <<'FTR'

Sizes are exact bytes, so a download can be checked against the number in this
table. Quad counts are the line count of the decompressed dump — the number the
loader will report when it finishes.

## Provenance and licence

Each dataset carries its own licence, stated in the Dataset Card embedded in the
source `.rete`. Read it without downloading the file:

```sh
rete card-url https://data.graphplaza.com/dblp/dblp.rete
```

The dumps are byte-for-byte derivable from those `.rete` files, and the `.rete`
files' own sources are mirrored in this bucket under `sources/`.
FTR
} > "$OUT"

echo "wrote $OUT ($(wc -c < "$OUT") bytes, $n dataset rows)"
[ "$LOCAL_ONLY" = "1" ] && exit 0

hf buckets cp "$OUT" "hf://buckets/$BUCKET/$PREFIX/README.md" || exit 5
landed="$(hf buckets ls "$BUCKET/$PREFIX/README.md" --recursive --json 2>/dev/null \
  | grep -o '"size": *[0-9]*' | grep -o '[0-9]*' | head -1)"
want="$(wc -c < "$OUT" | tr -d ' ')"
if [ "${landed:-}" = "$want" ]; then
  echo "VERIFY $PREFIX/README.md: $landed bytes confirmed"
else
  echo "FAIL   $PREFIX/README.md: bucket says '${landed:-absent}', wanted $want" >&2
  exit 6
fi
