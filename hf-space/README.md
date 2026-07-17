---
title: rete graph gateway
emoji: 🕸️
colorFrom: green
colorTo: indigo
sdk: docker
app_port: 7860
pinned: false
license: mit
---

# rete graph gateway — bytes, SPARQL, and MCP

One FastAPI app (Hugging Face Space, Docker SDK), three planes:

| Plane | Routes | What |
|---|---|---|
| **Bytes** | `/data/…`, `/files` | The original project-agnostic gateway: HTTP Range (`206`) + permissive CORS over everything under `/data`. |
| **SPARQL (REST)** | `/api/…` (+ `/docs`) | On-demand queries over the published `.rete` catalog or any range-readable URL. Lazy by default; fetched blocks persist in a disk cache. |
| **SPARQL 1.1 Protocol** | `/sparql/{dataset}` | One **standard W3C endpoint per dataset** — rdflib/SPARQLWrapper, Jena, YASGUI, or a federated `SERVICE <…>` clause talk to it unmodified. |
| **MCP** | `/mcp` | FastMCP server (streamable HTTP) exposing the same surface as tools for LLM apps — Claude, ChatGPT (`search`/`fetch` follow the connector contract), anything MCP. |

## The SPARQL plane

```sh
curl -s https://<space>/api/datasets                       # all published .rete + links
curl -s https://<space>/api/datasets/boe                   # catalog entry + embedded card
curl -s https://<space>/api/datasets/boe/card              # the Dataset Card alone
curl -s https://<space>/api/datasets/boe/schema            # classes + relations profile
curl -s https://<space>/api/datasets/boe/examples          # runnable example queries
curl -s "https://<space>/api/datasets/boe/search?q=Constitu"   # entity lookup
curl -s -X POST https://<space>/api/query \
  -H 'content-type: application/json' \
  -d '{"dataset": "boe", "query": "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 3"}'
```

SELECT answers carry W3C SPARQL-JSON `results` **and** a plain-values
`table`; every answer carries `stats` — the bytes physically fetched, split
into network vs disk-cache reads. ASK → `boolean`; CONSTRUCT/DESCRIBE →
`triples`. `reason: true` adds OWL 2 QL entailment by query rewriting.

**Two-tier lazy cache.** Every open is lazy (HTTP Range). Fetched byte
blocks (256 KiB) persist under `DATA_DIR/.rete-cache` — LRU-capped by
`RETE_CACHE_MAX_MB`, validated by length+ETag against the origin — so a
restarted Space answers warm queries with **zero** network reads. Resident
graph handles (an LRU of `RETE_MAX_HANDLES`) additionally keep decoded
dictionary chunks and index tiles in RAM.

## The SPARQL 1.1 Protocol plane

Every non-sharded dataset is a spec-conformant endpoint — and so is **any
range-readable `.rete` URL on the internet**, published by anyone: put the
full URL after `/sparql/`:

```sh
# A catalog dataset…
https://<space>/sparql/boe
# …or ANY .rete file, no registration needed:
https://<space>/sparql/https://example.org/their-graph.rete
```

```sh
# GET form; result is application/sparql-results+json (W3C shape)
curl -s "https://<space>/sparql/boe?query=SELECT%20*%20WHERE%20%7B%3Fs%20%3Fp%20%3Fo%7D%20LIMIT%203"

# Both POST forms work; CONSTRUCT/DESCRIBE answer N-Triples
curl -s -X POST https://<space>/sparql/boe --data-urlencode "query=ASK { ?s ?p ?o }"

# Without ?query=: the sd: service description (Turtle)
curl -s https://<space>/sparql/boe
```

```python
from SPARQLWrapper import SPARQLWrapper, JSON     # any standard client
sw = SPARQLWrapper("https://<space>/sparql/boe")
```

Each entry in `/api/datasets` carries its `sparql_endpoint`.

## SHACL validation

```sh
curl -s https://<space>/api/datasets/causenet/shapes   # curated shapes, run as-is
curl -s -X POST https://<space>/api/shacl \
  -H 'content-type: application/json' \
  -d '{"dataset": "causenet", "shapes": "@prefix sh: <http://www.w3.org/ns/shacl#> . …"}'
```

Validation is **lazy**: only the shapes' targets are fetched (a broad
`sh:targetClass` fetches many targets — scope shapes tightly). The report
comes back as `{conforms, results}` (or Turtle with `"format": "ttl"`).

## The MCP plane

Point any MCP client at `https://<space>/mcp/` (streamable HTTP, stateless,
no auth). Tools: `list_datasets`, `dataset_card`, `dataset_schema`,
`example_queries`, `sparql_query`, `find_entities`, `describe_entity`,
`validate_shacl`, `shacl_shapes`, plus `search`/`fetch` following the
ChatGPT connector contract. The server
instructions teach the workflow (card → schema → examples → query), so an
agent needs no out-of-band documentation — `.rete` files are self-describing.

## Natural-language `/ask` (optional)

`POST /api/ask {dataset, question}` runs a pydantic-ai agent that reads the
schema/examples, writes SPARQL, executes it, and returns
`{answer, sparql, table}`. Enabled only when `ASK_MODEL` is set (e.g.
`anthropic:claude-sonnet-5` + `ANTHROPIC_API_KEY`); otherwise 503.

## Configuration (env)

- `DATA_DIR` — served root (default `/data`); local `.rete` files here appear
  in the catalog as `local/<path>` and are queryable.
- `CATALOG_FILE` — published-dataset catalog (default `catalog.json`,
  exported from the playground catalog by
  `scripts/export_space_catalog.py` in the main repo).
- `RETE_CACHE_DIR` / `RETE_CACHE_BLOCK` / `RETE_CACHE_MAX_MB` — disk cache
  (defaults: `DATA_DIR/.rete-cache`, 256 KiB, 4096 MB).
- `RETE_MAX_HANDLES` / `RETE_ROW_CAP` / `RETE_QUERY_TIMEOUT_S` — query-plane
  guards (12 / 10 000 / 60 s).
- `ASK_MODEL` — pydantic-ai model id enabling `/api/ask`. For an
  OpenAI-compatible router (vLLM, HF, EPFL RCP) use the **`openai-chat:`**
  prefix (`openai-chat:<model>` + `OPENAI_BASE_URL` + `OPENAI_API_KEY`) —
  the plain `openai:` prefix targets the Responses API, which such routers
  don't serve (it 404s).
- `JWT_TOKEN`, `WEB_CONCURRENCY`, `THREADPOOL_TOKENS`, `BRANDING_FILE`,
  `EXTRA_CTYPES` — as in the byte gateway. Keep `WEB_CONCURRENCY=1` unless
  RAM allows duplicating resident graph handles per worker.

## Deploy

Push `Dockerfile`, `requirements.txt`, `app.py`, `rete_service.py`,
`rete_api.py`, `rete_mcp.py`, `rete_ask.py`, `catalog.json`,
`branding.json` + this README to a Docker Space. Persistent storage at
`/data` makes the disk cache survive restarts (and lets you drop local
`.rete` files in).
