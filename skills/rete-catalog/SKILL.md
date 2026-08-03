---
name: rete-catalog
description: Find, inspect, and open the published .rete datasets from the rete catalog — 60+ public knowledge graphs on range-readable hosting. Use when a task needs an existing dataset (query it, download it, federate it, wire it into an app) rather than building a new one.
---

# Use the rete dataset catalog

Every published dataset is a single `.rete` file on range-readable hosting.
The canonical URL scheme is:

```
https://data.graphplaza.com/<key>/<key>.rete
```

## 1. Discover what exists

Three equivalent sources, freshest first:

```sh
# Live catalog API (keys, descriptions, sizes, licenses, URLs, examples):
curl -s https://katospiegel-rete.hf.space/api/datasets | jq '.[].key'

# In this repo: the exported catalog (regenerate: scripts/export_space_catalog.py)
jq '.datasets[] | {key, size, license}' clients/relay/catalog.json

# Source of truth for the playground: web/playground-src/catalog.js
```

Expected hashes and sizes for every published file live in
`web/datasets.lock.json`; `scripts/check_dataset_catalog.py --all` probes
the real URLs against it.

## 2. Inspect before you query — the files are self-describing

Never guess vocabulary. A `.rete` carries its own card, schema profile, and
runnable example queries, and reading them costs a few range requests:

```sh
curl -s https://katospiegel-rete.hf.space/api/datasets/boe/card      # what it is
curl -s https://katospiegel-rete.hf.space/api/datasets/boe/schema    # classes + relations (exact IRIs)
curl -s https://katospiegel-rete.hf.space/api/datasets/boe/examples  # queries that run as-is
# CLI equivalents: rete card-url <url> · rete schema-url <url>
```

Copy IRIs verbatim from the schema; start from an example query and edit.

## 3. Open it from anywhere

Same file, every runtime — always lazy (only touched byte ranges are read):

```python
import rete_graph as rete                       # pip install rete-graph
g = rete.open("https://data.graphplaza.com/boe/boe.rete")
```

```js
import { open } from "rete-graph";              // npm install rete-graph
const g = await open("https://data.graphplaza.com/boe/boe.rete");
```

```r
g <- rete_open("https://data.graphplaza.com/boe/boe.rete")   # R package `rete`
```

```sh
rete sparql-url https://data.graphplaza.com/boe/boe.rete "SELECT … LIMIT 5"
```

No client at all? Each dataset is a **standard SPARQL 1.1 endpoint**
(`https://katospiegel-rete.hf.space/sparql/<key>` — or `/sparql/<any-full-
.rete-URL>` for unregistered files), so SPARQLWrapper/rdflib/YASGUI/Jena
and `SERVICE <…>` clauses work unmodified. The browser playground opens any
of them by key with zero install.

After a first query, sanity-check laziness: `g.stats()` should show a small
fraction of `fileLength` fetched.

## 4. Download when local is better

For repeated heavy analysis, one GET beats many ranges:

```sh
curl -sO https://data.graphplaza.com/boe/boe.rete
rete verify boe.rete && rete card boe.rete     # integrity + provenance
```

Local opens are lazy too (positional reads) — a big file costs no RAM.

## 5. Combine datasets

- Cross-dataset joins in one query: `rete federate` (CLI) or the
  playground's CROSS-SOURCE JOIN; datasets that share IRIs/PIDs join free
  (e.g. `bne` × `databnf` on VIAF, the scholar hub on DOI).
- From another triple store, federate via `SERVICE` against the endpoint —
  see docs/interop.md for when to federate vs when to dump-and-load.
- Sharded datasets (`shards` list in the catalog) are queried per shard by
  URL, or UNION-fanned by the playground.
