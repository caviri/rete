# Agentic interfaces — MCP, plugin & skills

rete has three agent-facing surfaces, all built on the same idea: a `.rete`
file is **self-describing** (card, schema, example queries travel inside the
file), so an agent can go from "what datasets exist?" to a correct SPARQL
query — and to validated, media-rich answers — without any out-of-band
documentation.

| Surface | What it is | For |
|---|---|---|
| **MCP server** | `https://katospiegel-rete.hf.space/mcp/` — 16 tools over the published catalog and any `.rete` URL, the graph view, skills and long queries as tasks | ChatGPT, Claude, any MCP client |
| **Desktop extension** | `rete.mcpb` — the whole engine as a one-click local install | Claude Desktop, incl. private graphs and offline work |
| **Claude Code plugin** | this repo, installable as a plugin + marketplace | Claude Code users: MCP + skills in two commands |
| **Skills** | four repo-aware playbooks under `skills/` | Claude Code; also readable as human docs |
| **semantic-site plugin** | `plugins/semantic-site`, in the same marketplace | Building static sites whose every page carries JSON-LD, a static API and WebMCP tools |

## The MCP server

One streamable-HTTP endpoint, stateless, **no authentication, no API key**
(the model lives in the client — the server only serves graphs). It runs
[rete-server](https://github.com/caviri/rete/tree/main/clients/server), read-only:
the authoring tools are off on the public copy. Run your own
(`docker run -p 7860:7860 ghcr.io/caviri/rete-server`) to have them, and a token
if you want one.

```
https://katospiegel-rete.hf.space/mcp/
```

The tools, grouped by what an agent does with them:

| Group | Tools |
|---|---|
| Discover | `list_datasets` · `dataset_card` · `dataset_schema` · `example_queries` |
| Query | `sparql_query` (SELECT/ASK/CONSTRUCT/DESCRIBE, `reason=true` for OWL 2 QL, any catalog key **or any `.rete` URL**; a big answer comes back as its first rows and a handle) · `result_rows` (page through it) · `find_entities` · `describe_entity` |
| Validate | `validate_query` (deterministic SPARQL lint: parse, prefixes, features, vocabulary probes vs a dataset, ontology/subclass checks) · `validate_shacl` (lazy over shape targets) · `shacl_shapes` (curated shapes) |
| Show | `graph_view` (query results, a neighbourhood or the schema as an interactive graph, table, cards or map in the conversation, for clients that render MCP Apps) · `graph_view_update` · `graph_view_state` |
| **Author** (own server only) | `suggest_vocabulary` (search LOV before minting IRIs) · `check_ontology` (parse + lint battery + reasoner smoke) · `build_rete` (RDF text → a served, immediately-queryable `.rete`) · `causal_diagram` (extracted claims → Mermaid + Graphviz SVG + a CauseNet-aligned graph) — see the [conversation experiments](fallacies.md) |
| ChatGPT connector contract | `search` · `fetch` |

Every answer carries `stats` — the bytes physically fetched — so laziness
stays observable. The server instructions teach the intended workflow
(card → schema → examples → query), and reads are disk-cached server-side.

Clients on MCP 2026-07-28 get two more things. Four **skills** travel with the
server (`query`, `build-rete`, `network-analysis`, `web-pages`): Claude Code
lists them and loads one when it is needed. And `sparql_query` and
`validate_shacl` can run as **tasks**, so a slow query does not hold up the
conversation (Claude Code, new claude.ai chats).

### Connect from ChatGPT

Two integration levels:

1. **Developer mode (all tools).** Settings → *Apps & Connectors* →
   enable *Developer mode* (under advanced settings) → *Create* a
   connector: any name, MCP server URL
   `https://katospiegel-rete.hf.space/mcp/`, authentication **None**.
   Enable it per-chat from the composer's tools menu.
2. **As a regular connector (search + deep research).** The server
   implements ChatGPT's `search`/`fetch` contract, so it also works as a
   plain connector: `search` matches datasets and entities, `fetch` returns
   the card/schema/examples or everything about one entity.

**Gotcha (hard-won):** ChatGPT snapshots the tool list when the connector
is created and does not refresh it on its own. After the server gains
tools, *refresh* the connector (or delete and re-add it) and start a new
chat — otherwise you keep the old tool list.

### Connect from Claude

- **Claude.ai (web/desktop):** Settings → *Connectors* → *Add custom
  connector* → the `/mcp/` URL, no auth. Available on paid plans.
- **Claude Code — the plugin way (recommended):** see below; installing
  the plugin wires the MCP automatically.
- **Claude Code — MCP only:**

  ```sh
  claude mcp add --transport http rete-graphs https://katospiegel-rete.hf.space/mcp/
  ```

### Connect from any other MCP client

Generic config (Cursor, Windsurf, custom hosts — field names vary
slightly per client):

```json
{
  "mcpServers": {
    "rete-graphs": {
      "type": "http",
      "url": "https://katospiegel-rete.hf.space/mcp/"
    }
  }
}
```

### Programmatic agents

An agent framework can reach a graph two ways: through this MCP server, or by
calling the `rete-graph` library **in process**, so the tools run against a
local path or a URL with no server in between —
[LangChain & Pydantic AI](agent-frameworks.md) is the tutorial for both,
with runnable scripts.

Verified with [pydantic-ai](https://ai.pydantic.dev) (2.x) — the full
tool loop over this server:

```python
from pydantic_ai import Agent
from pydantic_ai.mcp import MCPToolset

agent = Agent("anthropic:claude-sonnet-5",
              toolsets=[MCPToolset("https://katospiegel-rete.hf.space/mcp/")])
async with agent:
    result = await agent.run("Which datasets cover Spanish law? Query one of them.")
```

And with the [FastMCP](https://gofastmcp.com) client for direct calls:

```python
from fastmcp import Client

async with Client("https://katospiegel-rete.hf.space/mcp/") as c:
    tools = await c.list_tools()
    result = await c.call_tool("sparql_query", {
        "dataset": "boe",
        "query": "SELECT (COUNT(?s) AS ?n) WHERE { ?s a <http://data.europa.eu/eli/ontology#LegalResource> }",
    })
```

No MCP at all? The same surface is plain REST (`/api/…`, OpenAPI at
[`/docs`](https://katospiegel-rete.hf.space/docs)) and every dataset is a
standard [SPARQL 1.1 Protocol endpoint](interop.md)
(`/sparql/<key>` or `/sparql/<any-.rete-URL>`).

## The desktop extension (`rete.mcpb`)

The MCP server above is a *hosted* surface: it queries the published
catalog, and your own graphs are not on it. The
[MCP Bundle](https://github.com/modelcontextprotocol/mcpb) inverts that —
it installs the **whole engine on the user's machine**, so Claude Desktop
can query private `.rete` files with no network at all, and reach the
published catalog directly over HTTP Range with no server in between.

The format makes this unusually easy. MCPB's own guidance notes that Python
bundles cannot portably ship compiled dependencies and that binary bundles
need a build per platform; the rete engine is Rust **already compiled to
architecture-neutral wasm**, so the extension is a plain `node` bundle — two
JS files (the server and its query worker) plus one `.wasm` — that runs
unchanged on macOS, Windows and Linux with the Node runtime Claude Desktop
ships.

Eighteen tools, mirroring the hosted server's read surface plus local
authoring, query jobs and a graph view: `list_datasets`, `dataset_card`,
`dataset_schema`, `example_queries`, `sparql_query`, `start_query`,
`query_status`, `query_result`, `cancel_query`, `find_entities`,
`describe_entity`, `validate_shacl`, `build_rete`, `load_rdf`, `graph_view`,
`graph_view_update`, `graph_view_state` and `about`. Every `dataset` argument
takes a local file name, a catalog key, an `https://` URL to any published
`.rete`, `chat:<name>`, or the path of a file attached to the chat.
`dataset_card` also says what each graph supports (a schema, name search,
word search), read from its header and card without opening it.

**The graph view.** Claude Desktop renders [MCP Apps](https://github.com/modelcontextprotocol/ext-apps),
so `graph_view` shows a query's results, an entity's neighbourhood or a
dataset's schema as an interactive graph in the conversation: as dots, as boxes
listing each entity's properties, or as a class diagram. The user can change
labels, layers and layout (including a hive plot), search, expand and aggregate
nodes, switch to a table (pictures, IIIF manifests, geometries as small maps and
links in the cells, a chart of each column's values under its name, a click on a
bar to filter), to cards (a card per row, with pictures, IIIF, maps, audio, video
and 3D models), or to a map (WKT geometries or latitude and longitude over an
OpenStreetMap basemap, `.pmtiles` columns as layers), and send a selection back
to the chat. A query's comments can choose the tab (`# view: map`) and name the
columns (`# column ?v: Label — note`), as in
[example queries](dataset-card-reference.md#hints-in-example-queries). Claude changes the open view with
`graph_view_update` and reads what the user selected with `graph_view_state`.
The view fetches its data with tools only it can call, so the triples never
pass through the model. Details in `clients/mcpb/README.md`.

**Files shared in the chat.** Claude Desktop does not pass chat attachments
to an extension: the path Claude sees for an upload is in its own sandbox.
Given that path, the extension uses a file of that name in a granted folder;
otherwise `graph_view` opens a view that asks the user to choose or drop the
file, and sends it to the extension in parts, so a `.rete` (up to 512 MB) can
come from the chat too. For RDF the user attaches or pastes (Turtle, TriG,
N-Triples, N-Quads), the model can also send the text to `load_rdf`, which
builds a graph in memory, queryable by every tool as `chat:<name>` until the
extension restarts — no granted folder needed. Long files go in several parts;
`save_as` keeps the result as a `.rete` in a granted folder. A Jelly (`.jelly`)
file is read by `path` from a granted folder, by `load_rdf` and `build_rete`.

**Long queries do not time out.** Claude gives up on a tool call after 60 s,
and a query that visits thousands of scattered entities in a large published
graph (title and year for every work citing a paper, on the 60 GB Crossref
graph) takes minutes: each separate place in the file is a network round trip.
Every query therefore runs on a worker thread as a job. `sparql_query` waits
up to 40 s; a query still running by then answers with its `job_id` and keeps
going, `query_status` waits for it (45 s at most per call) and reports reads,
bytes and elapsed time, and `cancel_query` stops it. Up to three jobs run at
once, so a quick lookup never queues behind a slow one, and the server stays
responsive throughout (the engine's synchronous range reads used to block it
for the whole query). An answer shows 200 rows; `query_result` pages through
the rest by cursor or writes all of them to a CSV/TSV/JSON/N-Triples file in a
granted folder.

**Long extractions resume.** With `partition_by: "?work"`, `sparql_query` /
`start_query` split the query by that variable ([the rule](sparql.md#partition)):
its values are listed first, from the query's cheapest constant pattern on it,
then the query runs a few hundred values at a time, two chunks at once, each
sized to take about 15 s. Every finished chunk is kept on disk, so after a
cancel, a crash or a restart of Claude Desktop the same call carries on where
it stopped, and a call identical to one still running joins it. A split that
would change the answer (ORDER BY, an aggregate not grouped by the variable, a
variable only bound inside OPTIONAL) is refused, saying why. When the entities
come from another graph — the works citing a paper are in `crossref-citations`,
their titles in `crossref` — `keys_job` takes the values from the job that
listed them, so the model never copies thousands of IRIs into a query.

The decisive property is that **local files are read lazily too** — the same
byte-range path as a remote graph, via the JS client's `file://` reader — so
a multi-gigabyte graph on disk answers a selective query in megabytes and is
never loaded into memory.

### Try it now

**⬇ [Download rete.mcpb](https://github.com/caviri/rete/releases/latest/download/rete.mcpb)** (2.8 MB)
— then double-click it, or drag it into Claude Desktop.

That link always serves the build attached to the latest
[GitHub release](https://github.com/caviri/rete/releases/latest). Each release
also carries the same bytes under a pinned name (for example
[`rete-0.3.6.mcpb`](https://github.com/caviri/rete/releases/download/v0.3.6/rete-0.3.6.mcpb)),
built by the release workflow and listed in its `SHA256SUMS` with build
provenance — check them if you want to verify what you are installing before
you run it. (Builds up to 0.3.5 were also served from
`data.graphplaza.com/mcpb/`; those links still work but stop at 0.3.5.)

```sh
# or build it yourself — Docker only, no node needed
cd clients/mcpb && ./build.sh --test
```

At install time you choose which folders to expose. Leaving that empty is a
fine way to start: the published catalog still works and the extension can read
nothing on disk. Then ask Claude *"list the rete datasets, then show me the
classes in the BOE graph"*. To query your own graphs, point it at a folder
holding `.rete` files — reads and writes stay confined to the folders you
granted, compared on real paths so a symlink cannot escape. See
`clients/mcpb/README.md` for the build and test details.

## The Claude Code plugin

The repo doubles as a plugin **and** its own marketplace:

```
/plugin marketplace add caviri/rete
/plugin install rete-graph@rete
```

Installing wires up, in one step:

- the **MCP server** above (its tools available in every session), and
- the four **skills**, namespaced as `/rete-graph:<skill>`.

Versioning follows git — every push to `main` is a new plugin version, so
updates arrive without manual bumps. To try it without installing:
`claude --plugin-dir <checkout>`.

### semantic-site

The marketplace lists a second plugin, independent of rete:

```
/plugin install semantic-site@rete
```

It builds a static website from CSV, JSON, JSON-LD, NDJSON, N-Triples or
N-Quads. Every page is HTML content, schema.org JSON-LD, an entry in a static
JSON API (`api/index.json`, one file per list and per item, OpenAPI,
`llms.txt`), and a set of [WebMCP](https://webmachinelearning.github.io/webmcp/)
tools registered with `document.modelContext`, so an agent in the browser can
search, read and filter what the user sees. Its three skills: `semantic-site`
(inspect the data, write `site.json`, build, check, publish), `webmcp-tools`
(add WebMCP tools to any page) and `site-check` (verify a site before
publishing, including a run of every read-only tool). Details in
[`plugins/semantic-site`](https://github.com/caviri/rete/tree/main/plugins/semantic-site).

## The skills

Four repo-aware playbooks (in [`skills/`](https://github.com/caviri/rete/tree/main/skills),
loaded automatically by the plugin):

| Skill | Use it when |
|---|---|
| `rete-catalog` | "use an existing published dataset" — discover, read card/schema/examples, open from any client, download-and-verify, federate |
| `rete-clients` | "wire rete into a new project" — Python / Pyodide / JS / script-tag / wasm / Rust setup with verified first-query snippets |
| `rete-from-graph` | "turn this dataset/graph/ontology/endpoint into a `.rete`" — source → N-Triples → `rete build` → verify, with tested converter utilities |
| `rete-publish` | "make this `.rete` explorable in the playground" — companions → bucket → catalog → rebuild → verify |

Each is a `SKILL.md` with reference docs and working scripts — they read
fine as human documentation too.

## What an agent session looks like

A typical flow, entirely inside one chat, no rete-specific prompt
engineering:

1. `list_datasets` → picks `boe` (Spanish consolidated legislation).
2. `dataset_schema("boe")` → copies the exact ELI IRIs.
3. `example_queries("boe")` → adapts the citation-network example.
4. `sparql_query` with `reason=true` → counts norms including subclass
   entailment.
5. `validate_shacl` → checks an integrity contract over the result set.
6. `graph_view` → the citing and cited norms as an interactive graph in the
   conversation, which the user explores and selects from.
