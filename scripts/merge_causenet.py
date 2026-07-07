#!/usr/bin/env python3
"""Consolidate the two causenet datasets into one `causenet`:
keep causenet-full's rich entry (companions/rag/shacl/reach/provenance/examples),
rename its key to `causenet`, repoint its .rete URL to the *typed* file (which
carries the schema pyramid + text-index + ontology), and delete causenet-full-typed.
The typed .rete is the SAME 256M triples, just rebuilt with `--pyramid-algo types`."""
import re

CAT = "web/playground-src/catalog.js"
src = open(CAT, encoding="utf-8").read()

# 1. delete causenet-full-typed's entries (datasets/meta/extra one-liners; examples block)
src = re.sub(r'^    \{"key": "causenet-full-typed".*\n', '', src, flags=re.M)          # datasets
src = re.sub(r'^    "causenet-full-typed":\s+\{.*\n', '', src, flags=re.M)             # meta + extra (one-liners)
src = re.sub(r'^    "causenet-full-typed": \[.*?\n    \],?\n', '', src, flags=re.M | re.S)  # examples array (last entry closes with `]`, no comma)

# 2. rename causenet-full's map keys + datasets key -> causenet (NOT the R2 paths)
src = src.replace('"causenet-full":', '"causenet":')
src = src.replace('{"key": "causenet-full",', '{"key": "causenet",')

# 3. repoint the served .rete to the typed file (schema); companions stay on causenet-full/*
src = src.replace(
    '"https://data.graphplaza.com/causenet-full/causenet-full.rete"',
    '"https://data.graphplaza.com/causenet-full-typed/causenet-full-typed.rete"')

# 4. the served file is now the 6.39 GB typed build
src = src.replace('size: "4.56 GB"', 'size: "6.39 GB"')

open(CAT, "w", encoding="utf-8", newline="\n").write(src)
print("merged causenet-full-typed into causenet; removed causenet-full-typed")
