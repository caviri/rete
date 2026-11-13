# rete-server

A web app over `.rete` graph files: a REST API, one SPARQL 1.1 Protocol
endpoint per dataset, and an MCP server for agents, behind an optional token.
It replaces the relay that used to run on the Hugging Face Space.

The files stay where they are. By default the server answers for the
published catalog (`https://data.graphplaza.com/catalog.json`), reading each
file lazily over HTTP range, and it can also serve a folder of local files.

## Run it

```sh
docker run -p 7860:7860 -e RETE_TOKEN=change-me ghcr.io/caviri/rete-server
```

It ships as that image only, not on PyPI. From a checkout, without Docker:

```sh
pip install "./clients/server[ask]"     # [ask] only for /api/ask
rete-server --token change-me           # http://127.0.0.1:7860
```

With compose, a token from a Docker secret, your own files and a cache that
survives restarts:

```yaml
services:
  rete:
    image: ghcr.io/caviri/rete-server:latest
    ports: ["7860:7860"]
    environment:
      RETE_TOKEN_FILE: /run/secrets/rete_token
      RETE_CACHE_DIR: /cache
      RETE_WARM: open-pulse,boe
    secrets: [rete_token]
    volumes:
      - ./graphs:/data:ro
      - rete-cache:/cache
    mem_limit: 4g
secrets:
  rete_token:
    file: ./rete_token.txt
volumes:
  rete-cache:
```

Generate a token with `python -c "import secrets; print(secrets.token_urlsafe(32))"`.

## Endpoints

| Route | What |
|---|---|
| `/mcp` | MCP over streamable HTTP (stateless) |
| `/sparql/<dataset>` | SPARQL 1.1 Protocol: GET or POST, SPARQL-JSON or N-Triples |
| `/api/…` | REST: datasets, card, schema, examples, search, query, SHACL, status; documented at `/docs` |
| `/generated/<name>` | files built by the authoring tools |
| `/health` | liveness; the only route a token never covers |

`<dataset>` is a catalog key, `local/<path>` for a file in the data folder, or
a full public `.rete` URL (`/sparql/https://example.org/g.rete`).

Connect an agent:

```sh
claude mcp add --transport http rete https://<host>/mcp --header "Authorization: Bearer $RETE_TOKEN"
```

Query from anything that speaks SPARQL:

```sh
curl -s https://<host>/sparql/boe -H "Authorization: Bearer $RETE_TOKEN" \
  --data-urlencode 'query=SELECT * WHERE { ?s ?p ?o } LIMIT 3'
```

## Tokens

Set one or more tokens and every route except `/health` needs
`Authorization: Bearer <token>`. Any listed token works, so rotating means
adding the new one, moving clients over, then removing the old one. A token
file holds one per line (`#` starts a comment). `RETE_QUERY_TOKEN=1` also
accepts `?token=` for clients that cannot send a header; the parameter is
removed before the request is logged.

Without a token the server is open. It warns at startup when it binds a
non-loopback address with no token.

## Configuration

Every option is a flag and an environment variable; the flag wins.

| Variable | Flag | Default | |
|---|---|---|---|
| `RETE_HOST` / `RETE_PORT` | `--host` / `--port` | `127.0.0.1` / `7860` | the image binds `0.0.0.0` |
| `RETE_CATALOG` | `--catalog` | the published catalog | URLs or files, comma-separated; `none` for no catalog |
| `RETE_DATA_DIR` | `--data` | (none; `/data` in the image) | `.rete` files served as `local/<path>` |
| `RETE_TOKEN`, `RETE_TOKEN_FILE` | `--token`, `--token-file` | (none) | |
| `RETE_QUERY_TOKEN` | `--query-token` | off | accept `?token=` |
| `RETE_CACHE_DIR` | `--cache-dir` | (none) | keep fetched blocks of remote files on disk |
| `RETE_CACHE_MAX_MB` | `--cache-max-mb` | `2048` | |
| `RETE_WARM` | `--warm` | (none) | datasets to open at startup |
| `RETE_MAX_HANDLES` | `--max-handles` | `12` | graphs kept open |
| `RETE_ROW_CAP` | `--row-cap` | `10000` | |
| `RETE_QUERY_TIMEOUT_S` | `--query-timeout` | `60` | |
| `RETE_CATALOG_REFRESH_S` | `--catalog-refresh` | `600` | |
| `RETE_ALLOW_URLS` | `--no-urls` | on | queries against any public `.rete` URL |
| `RETE_ALLOW_PRIVATE_URLS` | | off | let `url=` reach private addresses |
| `RETE_AUTHOR` | `--no-author` | on | `build_rete`, `causal_diagram`, vocabulary and ontology checks |
| `RETE_ASK_MODEL` | `--ask-model` | (none) | a pydantic-ai model id, e.g. `anthropic:claude-sonnet-5-5` |
| `RETE_PUBLIC_URL` | `--public-url` | (none) | this server's own URL, for absolute links |
| `RETE_THREADS` | `--threads` | `32` | threads for engine calls |

## The catalog

A catalog is `{"datasets": [{"key", "label", "description", "url", …}]}`.
A relative `url` resolves against the catalog's own location, so a
`catalog.json` can sit next to the files it lists. The published one is
generated from the playground catalog by `scripts/export_catalog.py`;
the package carries a copy and uses it when the published one cannot be
read. URL catalogs are re-read every `RETE_CATALOG_REFRESH_S` (with
`If-None-Match`), and a failed refresh keeps the last good copy.

## How it stays fast

- One process. Every request shares the open graphs, so a warm query reads
  nothing from the network. Concurrency comes from threads: the engine
  releases the GIL while it works.
- A graph is opened once, even when many first requests arrive together.
- An answer is shaped once: each distinct term is converted once, the SPARQL
  endpoint builds only the W3C form, and JSON is written with orjson.
- MCP answers go back as plain JSON, not as a one-event stream.
- Queries on one graph still take turns, because the engine's check that a
  range fetch failed is per graph, not per query. A waiting request gives up
  with 503 after the query timeout instead of piling up.

`bench/` measures all of this against `rete serve`; see `bench/README.md`.

## Tests

```sh
docker build -t rete-server-test clients/server
docker run --rm -v "$PWD/clients/server:/src" -w /src rete-server-test sh -c \
  'pip install -q ".[test]" && python -m pytest tests -q'
```

No network and no LLM: datasets are built into a temporary folder, and the
`/api/ask` agent runs on pydantic-ai's scripted `FunctionModel`.
