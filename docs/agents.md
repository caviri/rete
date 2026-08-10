# Agentic Interfaces: MCP, Plugins, & Skills

Because `.rete` files are entirely **self-describing** (containing their own schemas, summaries, and example queries), AI agents can discover datasets, write perfect SPARQL queries, and analyze data *without any external documentation*.

We expose this power through three robust agentic surfaces:

| Surface | What it is | Target Audience |
|---|---|---|
| **MCP Server** | A hosted endpoint with 18 specialized tools covering the published catalog and any `.rete` URL. | ChatGPT, Claude, and general MCP clients. |
| **Desktop Extension** | (`rete.mcpb`) A local, single-click install of the engine for querying private graphs offline. | Claude Desktop users. |
| **Claude Code Plugin** | An installable plugin that wires up MCP tools and agent "skills" instantly. | CLI Developers using Claude Code. |
| **Agent Skills** | Four repo-aware Markdown playbooks teaching agents how to work with Rete. | AI Agents (and humans!). |

---

## 1. The Hosted MCP Server

We host a stateless, streamable-HTTP server. **No authentication or API keys required.**

**Endpoint:** `https://katospiegel-rete.hf.space/mcp/`

### The Toolkit
The server exposes 18 tools designed for autonomous discovery and analysis:

- **Discover:** `list_datasets`, `dataset_card`, `dataset_schema`, `example_queries`.
- **Query:** `sparql_query` (Run SELECT/ASK/CONSTRUCT on any URL! Supports OWL 2 QL), `find_entities`, `describe_entity`.
- **Validate:** `validate_query` (A deterministic SPARQL linter), `validate_shacl`, `shacl_shapes`.
- **Author:** `suggest_vocabulary` (LOV search), `check_ontology`, `build_rete` (Build a `.rete` file instantly), `causal_diagram`.
- **Media:** `embed_media`, `media_preview`.

*Every response includes network statistics (`bytes` fetched) so the agent is always aware of the lazy-loading efficiency.*

### How to Connect

**From ChatGPT (Developer Mode):**
Go to Settings → Apps & Connectors → Enable *Developer mode*. Create a connector pointing to the URL above. (Auth: None). 
*Gotcha:* ChatGPT snapshots the tool list at creation. If we add new tools, you must recreate the connector!

**From Claude (Web/Desktop):**
Go to Settings → Connectors → Add custom connector → Paste the URL.

**Using Agent Frameworks (Python):**
You can connect programmatic agents like Pydantic AI directly to the server:

```python
from pydantic_ai import Agent
from pydantic_ai.mcp import MCPToolset

# Give Claude full access to the Rete ecosystem!
agent = Agent("anthropic:claude-sonnet-5",
              toolsets=[MCPToolset("https://katospiegel-rete.hf.space/mcp/")])
```

---

## 2. The Desktop Extension (`rete.mcpb`)

Want to let Claude Desktop query your private, local `.rete` files without uploading them? Use the MCP Bundle extension!

It installs the **entire WebAssembly engine on your machine**. It accesses local files using the exact same lazy byte-range reader it uses for remote HTTP files, meaning Claude can analyze a 10 GB file instantly without exhausting your RAM.

**[⬇ Download rete.mcpb](https://data.graphplaza.com/mcpb/rete.mcpb)** (1.4 MB)

1. Download the file.
2. Drag it into Claude Desktop.
3. Choose which local folders Claude is allowed to access. (Leave blank to only allow remote public datasets).

---

## 3. The Claude Code Plugin & Skills

If you use Claude Code in the terminal, this repository doubles as a plugin and a marketplace!

Install it to instantly wire up the MCP server and specialized agent skills:

```sh
/plugin marketplace add caviri/rete
/plugin install rete-graph@rete
```

### The AI Skills
The plugin loads four specialized playbooks (located in `skills/`) that teach Claude how to execute complex Rete workflows:

- **`rete-catalog`:** Guides the agent to discover, validate, and federate existing published datasets.
- **`rete-clients`:** Helps the agent wire Rete into new Python/JS/Rust projects with working code snippets.
- **`rete-from-graph`:** Teaches the agent to convert raw RDF data into a highly compressed `.rete` file.
- **`rete-publish`:** Teaches the agent the exact workflow for publishing a `.rete` file to the web.

---

## 4. What an Agent Session Looks Like

Because of Rete's self-describing architecture, a typical autonomous AI session requires zero prompting from you:

1. **Agent:** Runs `list_datasets` and finds `boe` (Spanish legislation).
2. **Agent:** Runs `dataset_schema("boe")` to understand the exact classes and IRIs used in the file.
3. **Agent:** Runs `example_queries("boe")` to study how human authors query the dataset.
4. **Agent:** Crafts a perfect `sparql_query` using `reason=true` to count regulations (leveraging subclass entailment).
5. **Agent:** Uses `embed_media` to generate a self-contained HTML report featuring embedded PDF references pulled straight from the query!
