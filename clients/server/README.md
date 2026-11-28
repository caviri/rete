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
| `/mcp` | MCP over streamable HTTP (stateless), with the graph view |
| `/sparql/<dataset>` | SPARQL 1.1 Protocol: GET or POST, SPARQL-JSON or N-Triples |
| `/api/…` | REST: datasets, card, schema, examples, search, query, SHACL, status; documented at `/docs` |
| `/generated/<name>` | files built by the authoring tools |
| `/` | the home page (a static shell, open without a token; see below) |
| `/api/uploads` | `POST` a file, it becomes a dataset (needs authoring on) |
| `/health` | liveness, open without a token |

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

## The home page

`/` is a small web app for people: the datasets with a search box, a card for
the one you pick, a SPARQL box with starter queries, the lines to connect an
agent, and a drop zone. It is plain HTML, CSS and JS in `rete_server/home/`,
with no build step and nothing loaded from another site.

A browser cannot send a header when it navigates, so the page itself (`/`,
`/home/home.css`, `/home/home.js`, `/home/tools.js`) loads without a token. It
holds no data. When the server has a token the page asks for it, keeps it in
`sessionStorage` (this tab only; `/?token=…` works too and is removed from the
address bar) and sends it as a bearer token with every call. Everything else,
the API, SPARQL, MCP and `/docs`, still needs the token.

Drop a Turtle, N-Triples, N-Quads, TriG, JSON-LD, RDF/XML or Jelly file, or a
`.rete`, on the page. The server builds it, stores it as a `generated/…`
dataset and the page opens it with a starter query. The same endpoint takes a
file from a terminal, so a local file never has to pass through a conversation:

```sh
curl --data-binary @file.ttl "https://<host>/api/uploads?name=file.ttl" -H "Authorization: Bearer $RETE_TOKEN"
```

The body is the raw file, capped at `RETE_UPLOAD_MAX_MB` (413 above; RDF text is
also limited to 32 MB because it is built in memory). It answers the dataset
key, its card and a starter query. A file that is not valid RDF is a 400 that
names the problem; with `RETE_AUTHOR=0` it is a 403. `GET /api/uploads` tells a
client whether uploads are on.

In a browser with WebMCP the page offers its own tools to the browser's agent,
named as in the playground: `list_datasets`, `describe_dataset`,
`get_dataset_card`, `get_schema`, `list_examples`, `run_sparql`,
`find_entities`, `open_dataset`, `show_query`, `upload_info`, `get_activity`.
Each call shows in a small label at the bottom of the page while it runs.
Without WebMCP nothing happens; scripts can call the same tools through
`window.reteHomeTools`.

## The graph view

`graph_view` opens an interactive graph of a query's results, an entity's
neighbourhood or a dataset's schema, right in the conversation. The user can
label, group, lay out, search, expand and edit it. Besides the graph there
are tabs for the rows as a table (pictures and links drawn in the cells, a
chart of each column), as cards, and on a map when the rows hold geometries
or coordinates. `options.view` picks the tab; so does a comment line in the
query (`# view: map`). `options.card_fields` says how a column is drawn on
the cards. The layout `hive` puts nodes on a few radial axes (`hive_axes`,
`hive_order`). Claude changes it with `graph_view_update` and reads the selection with
`graph_view_state`.

The server renders nothing. It sends the page (`ui://rete/graph-view.html`)
as an [MCP App](https://github.com/modelcontextprotocol/ext-apps) and the
client draws it, exactly as with the Claude Desktop extension: it is the same
page, copied from `clients/mcpb` by `clients/mcpb/build-explorer.mjs`. The
page asks for its data with six app-only tools (`graph_view_sync`, `…_labels`,
`…_expand`, `…_search`, `…_edit`, `…_upload`), so the data never passes
through the model. A client without MCP Apps gets the text answers only.

The cards load pictures, audio, video and 3D models from the sites a
value names, but a client only allows the origins the page declares. The
server declares Wikimedia, graphplaza.com, Hugging Face, the Internet Archive
and GitHub's content hosts; `RETE_VIEW_MEDIA_DOMAINS` replaces that list
(comma separated, like `https://*.example.org`, or `none` for no network).

A file attached to the chat sits in Claude's sandbox, out of the server's
reach. Given its path (`/mnt/user-data/uploads/…`), `graph_view` asks the user
to choose or drop the file in the view; it becomes a `generated/…` dataset
that every tool can query. That goes with authoring, so `--no-author` turns
it off, and `RETE_UPLOAD_MAX_MB` caps the file (RDF text at 32 MB at most).

Views live in the server's memory, the newest 200, under ids nobody can
guess.

## Skills

The server also serves four skills
(Skills over MCP, SEP-2640, extension `io.modelcontextprotocol/skills`). A
client that supports them lists the names and descriptions with `skills/list`
and loads a skill's files only when it uses it:

| Skill | For |
|---|---|
| `query` | exploring and querying a graph with SPARQL: the workflow, fast and slow shapes, federation, known engine issues |
| `build-rete` | building a complete `.rete`: model, card, starter queries, SHACL shapes, build, verify (server tools or the CLI) |
| `network-analysis` | degree, hubs, neighbourhoods, paths, projections, centrality and communities from an exported edge list |
| `web-pages` | pages and sites from a `.rete`: live queries in the browser, static pages with JSON-LD, WebMCP tools, link previews |

They are plain folders in `rete_server/skills/` (`SKILL.md` plus
`references/`), read once at startup and shipped in the wheel and the image.
Clients that do not support skills, such as claude.ai chat, still get the short
workflow in the server's instructions, and any client can read the files as
`skill://<name>/…` resources. `tests/test_skills.py` checks the protocol (2026-07-28
`skills/list` and `skills/get`, and the 2025-11-25 handshake), the size limits
Claude Code applies (100 KB a file, 400 KB a skill), that every tool a skill
names exists, and that every SPARQL block in them parses and runs.

## Tokens

Set one or more tokens and every route except `/health` and the home page's
static shell needs
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
| `RETE_TASK_TIMEOUT_S` | `--task-timeout` | `900` | time limit of a task |
| `RETE_TASK_ROW_CAP` | `--task-row-cap` | `100000` | most rows a task may return |
| `RETE_TASK_WORKERS` | `--task-workers` | `2` | tasks that run at once |
| `RETE_INLINE_ROWS` | `--inline-rows` | `200` | more rows than this are stored, not returned |
| `RETE_INLINE_BYTES` | `--inline-bytes` | `60000` | the same, in bytes of JSON |
| `RETE_RESULTS_DIR` | `--results-dir` | `/tmp/rete-results` | where stored answers live |
| `RETE_RESULTS_MAX_MB` | `--results-max-mb` | `512` | size cap, oldest evicted first |
| `RETE_RESULTS_TTL_S` | `--results-ttl` | `3600` | how long a stored answer lives |
| `RETE_CATALOG_REFRESH_S` | `--catalog-refresh` | `600` | |
| `RETE_ALLOW_URLS` | `--no-urls` | on | queries against any public `.rete` URL |
| `RETE_ALLOW_PRIVATE_URLS` | | off | let `url=` reach private addresses |
| `RETE_AUTHOR` | `--no-author` | on | `build_rete`, `causal_diagram`, vocabulary and ontology checks, files opened in a graph view |
| `RETE_UPLOAD_MAX_MB` | `--upload-max-mb` | `64` | largest file opened in a graph view |
| `RETE_ASK_MODEL` | `--ask-model` | (none) | a pydantic-ai model id, e.g. `anthropic:claude-sonnet-5-5` |
| `RETE_PUBLIC_URL` | `--public-url` | (none) | this server's own URL, for absolute links |
| `RETE_THREADS` | `--threads` | `32` | threads for engine calls |

## Long queries and large results

**Big answers stay out of the conversation.** When `sparql_query` over MCP
would return more than 200 rows (or about 60 kB of JSON), the server keeps the
answer and sends back a summary instead: the columns, the total row count, the
first 20 rows, `truncated: true` and a `result` handle, plus a `resource_link`
to `rete://results/<id>`. Small answers come back exactly as before. The rest
is read in pages with the `result_rows` tool or by reading the resource
(`rete://results/<id>?offset=200&limit=200`), which works in every client. To
get the whole thing onto disk, download it:

```sh
curl -H "Authorization: Bearer $RETE_TOKEN" -o answer.csv \
  "https://<host>/api/results/<id>?format=csv"
```

`format` is `json`, `srj` (W3C SPARQL results), `csv` or `tsv` for SELECT and
`nt` (or `json`) for CONSTRUCT / DESCRIBE. The link in the handle uses the
host the client called, or `RETE_PUBLIC_URL`; set that behind a TLS proxy.
Stored answers live in `RETE_RESULTS_DIR`, expire after an hour, and the oldest
go first when the folder passes its size cap. They are per server, not per
user: anyone with a token and the unguessable id can read one.

**Slow queries run as tasks.** `sparql_query`, `validate_shacl` and
`build_rete` support the MCP tasks extension (`io.modelcontextprotocol/tasks`,
protocol 2026-07-28). A client that declares it (Claude Code 2.1.304 and
later, claude.ai) gets a task back at once and polls it; `statusMessage` says
which dataset is being queried and for how long. Any other client, including
the old 2025-11-25 handshake, gets the ordinary synchronous answer. A task has
its own time limit, `RETE_TASK_TIMEOUT_S` (900 s), instead of the 60 s one, and
may return up to `RETE_TASK_ROW_CAP` rows (100000, into the result store). A
call that fails comes back as a `completed` task whose result has `isError`,
the same error a normal call gives. A server runs `RETE_TASK_WORKERS` (2) tasks
at once; the rest wait their turn. Tasks live in the server's memory, so a
restart loses the ones in flight.

**A task does not block quick calls.** A graph answers one query at a time (see
below), so a task that ran for minutes would make every quick call on that
dataset wait and then fail as busy. Tasks therefore open their own second copy
of the graph: a quick call beside a running task answers in about 150 ms
(measured on the test dataset), where behind another quick query it waits for
it and fails after the query timeout. The price is a second set of cached
tiles per dataset in use. Two tasks on one dataset still take turns, and the
second says it is waiting for another long query.

**Cancelling does not stop the engine.** `tasks/cancel` marks the task
cancelled and throws its result away, but the engine cannot be interrupted, so
the query it is running goes on to the end and holds that copy of the graph
until it does. A runaway query is best prevented with a `LIMIT` and a lower
`RETE_TASK_TIMEOUT_S`; the limit is checked when the query returns, not while
it runs.

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

`tests/ui` drives the graph view in Chromium against a live server; it runs
only with `RETE_UI_TEST=1`, in the Playwright image (the command is at the
top of `tests/ui/test_view_ui.py`).
