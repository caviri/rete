#!/bin/bash
# Loop over every dataset, run the generic label query, write raw/<key>.txt.
# Resumable (skips existing). Remote (giant) queries are timeout-capped.
Q='SELECT DISTINCT ?s ?label WHERE { { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label } UNION { ?s <http://www.w3.org/2004/02/skos/core#prefLabel> ?label } UNION { ?s <http://purl.org/dc/terms/title> ?label } UNION { ?s <http://schema.org/name> ?label } UNION { ?s <http://xmlns.com/foaf/0.1/name> ?label } } LIMIT 8000'
mkdir -p data/rag/raw
while IFS=$'\t' read -r key local ref; do
  [ -z "$key" ] && continue
  if [ -f "data/rag/raw/$key.txt" ]; then echo "skip $key"; continue; fi
  if [ "$local" = "true" ]; then
    ./target/release/rete sparql "$ref" "$Q" > "data/rag/raw/$key.txt" 2>/dev/null || echo "  ERR $key"
  else
    timeout 700 ./target/release/rete sparql-url "$ref" "$Q" > "data/rag/raw/$key.txt" 2>/dev/null || echo "  ERR/timeout $key"
  fi
  echo "done $key: $(grep -c '?s=' "data/rag/raw/$key.txt" 2>/dev/null) rows"
done < data/rag/datasets.tsv
echo ALL_DONE
