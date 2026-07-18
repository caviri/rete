# Agentic interfaces — MCP, plugin & skills

rete has three agent-facing surfaces, all built on the same idea: a `.rete`
file is **self-describing** (card, schema, example queries travel inside the
file), so an agent can go from "what datasets exist?" to a correct SPARQL
query — and to validated, media-rich answers — without any out-of-band
documentation.

| Surface | What it is | For |
|---|---|---|
| **MCP server** | `https://katospiegel-rete.hf.space/mcp/` — 13 tools over the published catalog and any `.rete` URL | ChatGPT, Claude, any MCP client |
| **Claude Code plugin** | this repo, installable as a plugin + marketplace | Claude Code users: MCP + skills in two commands |
| **Skills** | four repo-aware playbooks under `skills/` | Claude Code; also readable as human docs |

## The MCP server

One streamable-HTTP endpoint, stateless, **no authentication, no API key**
(the model lives in the client — the server only serves graphs):

```
https://katospiegel-rete.hf.space/mcp/
```

The tools, grouped by what an agent does with them:

| Group | Tools |
|---|---|
| Discover | `list_datasets` · `dataset_card` · `dataset_schema` · `example_queries` |
| Query | `sparql_query` (SELECT/ASK/CONSTRUCT/DESCRIBE, `reason=true` for OWL 2 QL, any catalog key **or any `.rete` URL**) · `find_entities` · `describe_entity` |
| Validate | `validate_shacl` (lazy over shape targets) · `shacl_shapes` (curated shapes) |
| **Author** | `suggest_vocabulary` (search LOV before minting IRIs) · `check_ontology` (parse + lint battery + reasoner smoke) · `build_rete` (RDF text → a served, immediately-queryable `.rete`) — see the [fallacy experiment](fallacies.md) |
| Media | `embed_media` (URLs → base64 data URIs, images recompressed to WebP) · `media_preview` (representative image of a PDF / video frame / IIIF / HTML page) |
| ChatGPT connector contract | `search` · `fetch` |

Every answer carries `stats` — the bytes physically fetched — so laziness
stays observable. The server instructions teach the intended workflow
(card → schema → examples → query), and reads are disk-cached server-side.

### Connect from ChatGPT

Two integration levels:

1. **Developer mode (all 13 tools).** Settings → *Apps & Connectors* →
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

## The Claude Code plugin

The repo doubles as a plugin **and** its own marketplace:

```
/plugin marketplace add caviri/rete
/plugin install rete-graph@rete
```

Installing wires up, in one step:

- the **MCP server** above (13 tools available in every session), and
- the four **skills**, namespaced as `/rete-graph:<skill>`.

Versioning follows git — every push to `main` is a new plugin version, so
updates arrive without manual bumps. To try it without installing:
`claude --plugin-dir <checkout>`.

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
6. `media_preview` on a IIIF manifest or PDF the query surfaced →
   `embed_media` → a self-contained HTML report with the evidence inlined.
