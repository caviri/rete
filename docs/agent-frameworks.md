# Agent frameworks — LangChain & Pydantic AI

[MCP](agents.md) is one way to give a model a `.rete` graph, and it is the
right one when you want the published catalog with no code. But an agent
framework can also talk to the file **directly**: `rete-graph` is an ordinary
Python library, so five short methods become five tools, and the agent queries
a local path or an `https://` URL with no server anywhere in between.

This page is a tutorial for that, in **Pydantic AI** and **LangChain** — both
verified end to end against a live model and the published
[`boe`](https://data.graphplaza.com/boe/boe.rete) graph. The runnable scripts
live in
[`clients/python/examples/agents/`](https://github.com/caviri/rete/tree/main/clients/python/examples/agents).

| Route | The agent talks to | Choose it when |
|---|---|---|
| **In-process tools** | the `.rete` file, via `rete-graph` | your own graphs, private or offline data, one graph per agent, no infrastructure |
| **MCP** | the [hosted server](agents.md) (18 tools) | you want the whole published catalog, plus authoring/media tools, with ~5 lines of code |
| **SPARQL endpoint** | `rete serve` or the [Space](interop.md) | you already have RDF tooling that speaks the SPARQL 1.1 protocol |

They compose: an agent can hold in-process tools for the graph you own and an
MCP toolset for everything published.

## Why a `.rete` is unusually easy to hand an agent

A model cannot query a graph it does not understand, and the usual fix — hand-
written schema documentation in the system prompt — rots the moment the data
changes. A `.rete` file carries its own description:

- **`card()`** — title, description, licence, counts, vocabularies.
- **`schema()`** — the classes and subject→predicate→object relations *actually
  present*, with counts, from the pyramid baked in at build time. The IRIs the
  model must copy are facts about this file, not prose about it.
- **`examples()`** — runnable SPARQL the dataset ships with: the graph's own
  idioms, already correct.

So the discovery loop — card → schema → examples → query — needs no
graph-specific prompt engineering, and the same agent code works against any
`.rete` file you point it at.

The second property is cost. Reads are HTTP byte ranges, so a tool call is
priced by the *query*, not the file. Opening the 2.27 GB `dblp` graph and
running a query over it fetched **10.7 MB in 20 requests — 0.47% of the file**
(measured; `stats()` reports it). Nothing is downloaded, nothing is indexed,
there is no database to run.

## Install

```sh
pip install rete-graph                              # the engine + client
pip install "pydantic-ai-slim[anthropic]"           # ... for Pydantic AI
pip install langchain langchain-anthropic           # ... for LangChain
```

## The tool layer

Shared by both frameworks — five methods over one graph, each returning a JSON
string, from
[`rete_tools.py`](https://github.com/caviri/rete/blob/main/clients/python/examples/agents/rete_tools.py):

```python
import json
import rete_graph as rete


class ReteGraphTools:
    def __init__(self, source: str, max_rows: int = 100):
        self.max_rows = max_rows
        self.graph = rete.open(source)          # local path or https:// URL

    def dataset_card(self) -> str:
        """What this graph is: title, description, licence, size, vocabularies."""
        card = self.graph.card() or {}
        keep = ("title", "description", "license", "source", "triple_count",
                "term_count", "vocabularies", "languages")
        return json.dumps({k: card[k] for k in keep if card.get(k)}
                          | {"info": self.graph.info()})

    def dataset_schema(self) -> str:
        """The classes and relations actually present. Copy these IRIs verbatim."""
        schema = self.graph.schema()
        classes = sorted(schema["classes"], key=lambda c: -c[1])[:40]
        relations = sorted(schema["relations"], key=lambda r: -r[3])[:60]
        return json.dumps({
            "classes": [{"class": c[0], "instances": c[1]} for c in classes],
            "relations": [{"subject_class": r[0], "predicate": r[1],
                           "object_class": r[2], "count": r[3]} for r in relations],
        })

    def example_queries(self) -> str:
        """Runnable SPARQL the dataset ships with — the graph's own idioms."""
        return json.dumps([{"question": e.get("question"), "sparql": e["sparql"]}
                           for e in (self.graph.examples() or [])[:8]])

    def find_entities(self, text: str) -> str:
        """Resolve a name to entity IRIs. Use before querying for a named thing."""
        hits = list(self.graph.prefix_search(text) or [])
        if not hits:                             # only built files have a text index
            hits = list(self.graph.text_search(text) or [])
        return json.dumps([{"label": h[0], "iri": h[1]} for h in hits[:20]])

    def sparql_query(self, query: str) -> str:
        """Run SPARQL 1.1 over the graph. Errors come back as {"error": ...}."""
        try:
            result = self.graph.query(query)
        except (ValueError, RuntimeError) as exc:
            return json.dumps({"error": str(exc)})   # the model reads this and retries
        if isinstance(result, bool):
            return json.dumps({"kind": "ask", "boolean": result})
        rows = [{k: v.to_python() for k, v in row.items()}
                for row in result[: self.max_rows]]
        return json.dumps({"kind": "table", "rows": rows}, default=str)
```

Three decisions in there matter more than they look, and each is explained
under [Gotchas](#gotchas) below: the schema is **trimmed**, query errors are
returned as **data** rather than raised, and the graph handle is opened
**once** and reused.

The system prompt is the other half — it teaches the loop, not the graph:

```python
SYSTEM_PROMPT = """\
You answer questions about ONE RDF knowledge graph by writing and running
SPARQL. Ground every answer in query results — never guess.

Method:
1. dataset_card, then dataset_schema. Copy class and predicate IRIs exactly
   from the schema; never invent a namespace or a prefix.
2. example_queries shows how this particular graph is meant to be queried.
3. If the question names a specific thing, resolve it with find_entities first.
4. sparql_query, then read the rows and refine. Always use LIMIT (<= 200).
   If a query errors, fix it from the message and try again.

Answer in prose, and state the SPARQL you ran.
"""
```

## Pydantic AI

Pydantic AI takes plain callables as tools and derives the schema from the
signature and the description from the docstring — so the bound methods go in
as they are.

```python
import asyncio
from pydantic_ai import Agent
from rete_tools import SYSTEM_PROMPT, ReteGraphTools

tools = ReteGraphTools("https://data.graphplaza.com/boe/boe.rete")

agent = Agent(
    "anthropic:claude-sonnet-5",
    system_prompt=SYSTEM_PROMPT,
    tools=[tools.dataset_card, tools.dataset_schema, tools.example_queries,
           tools.find_entities, tools.sparql_query],
)

result = asyncio.run(agent.run(
    "How many laws are in this graph? Show me three with their titles."
))
print(result.output)
print(tools.read_stats())     # {'fileLength': 6958628, 'bytes': 3065519, 'requests': 18}
```

The run above, verbatim from the model, after it read the schema and wrote its
own query:

> Result: `count = 40937`
>
> ```sparql
> SELECT (COUNT(DISTINCT ?law) AS ?count)
> WHERE { ?law a <http://data.europa.eu/eli/ontology#LegalResource> . }
> ```

The ELI IRI is not in the prompt: the agent took it from `dataset_schema`,
where `eli:LegalResource` is the largest class with exactly 40,937 instances.

Want structured output instead of prose? Add `output_type=` with a Pydantic
model — the pattern the Space's own
[`rete_ask.py`](https://github.com/caviri/rete/blob/main/clients/relay/rete_ask.py)
uses to return `{answer, sparql, table}`.

## LangChain

Same tools, wrapped with `StructuredTool.from_function`, driven by
`create_agent` (which returns a LangGraph graph, so checkpointers, streaming,
and human-in-the-loop interrupts are available on it).

```python
from langchain.agents import create_agent
from langchain.chat_models import init_chat_model
from langchain_core.tools import StructuredTool
from rete_tools import SYSTEM_PROMPT, ReteGraphTools

tools = ReteGraphTools("https://data.graphplaza.com/boe/boe.rete")
methods = [tools.dataset_card, tools.dataset_schema, tools.example_queries,
           tools.find_entities, tools.sparql_query]
toolkit = [StructuredTool.from_function(m, name=m.__name__) for m in methods]

agent = create_agent(init_chat_model("claude-sonnet-5", model_provider="anthropic"),
                     toolkit, system_prompt=SYSTEM_PROMPT)

result = agent.invoke({"messages": [
    {"role": "user", "content": "Which three laws here are the oldest?"}
]})
print(result["messages"][-1].content)
```

Verbatim from that run — a graph the model had never seen, three tool calls in:

> | Title | Date |
> |---|---|
> | Ley de 17 de junio de 1855 haciendo extensiva a los sucesores… | 1855‑06‑17 |
> | Ley del Notariado de 28 de mayo de 1862. | 1862‑05‑28 |
> | Ley de 18 de junio de 1870 estableciendo reglas para el ejercicio de la gracia de indulto. | 1870‑06‑18 |

## Your own graphs, offline

Nothing above is specific to a published dataset. Point the same agent at a
local file and no packet leaves the machine:

```python
tools = ReteGraphTools("data/my-graph.rete")
```

Or build one first — from Turtle, N-Triples, or an rdflib graph — and hand the
agent the result ([the build tutorial](python-build-tutorial.md) has the
details):

```python
import rete_graph as rete

rete.Builder().add_file("notes.ttl").card(title="Notes").export("notes.rete")
tools = ReteGraphTools("notes.rete")
```

Local files are read lazily too, so a multi-gigabyte graph on disk is never
loaded into memory.

## The MCP route, in both frameworks

When you want the whole published catalog — plus `list_datasets`,
`describe_entity`, SHACL validation, vocabulary suggestion, `build_rete` and
the media tools — point the framework's MCP adapter at the hosted server. No
tool definitions to write.

```sh
pip install "pydantic-ai-slim[mcp]"      # or, for LangChain:
pip install langchain-mcp-adapters
```

```python
# Pydantic AI
from pydantic_ai import Agent
from pydantic_ai.mcp import MCPToolset

agent = Agent("anthropic:claude-sonnet-5",
              toolsets=[MCPToolset("https://katospiegel-rete.hf.space/mcp/")])
async with agent:
    result = await agent.run("Which datasets cover Spanish law? Query one of them.")
```

```python
# LangChain
from langchain.agents import create_agent
from langchain_mcp_adapters.client import MultiServerMCPClient

client = MultiServerMCPClient({"rete": {
    "transport": "streamable_http",
    "url": "https://katospiegel-rete.hf.space/mcp/"}})
tools = await client.get_tools()              # 18 tools, no auth
agent = create_agent("anthropic:claude-sonnet-5", tools)
result = await agent.ainvoke({"messages": [
    {"role": "user", "content": "Count the laws in the rete dataset 'boe'."}]})
```

Both were verified against the live server. The trade-off against in-process
tools is the obvious one: MCP gives you the catalog and a much larger tool
surface, at the cost of a network hop per call and someone else's server;
in-process tools give you your own files, offline, at library speed.

## The SPARQL-endpoint route

If your stack already speaks SPARQL, skip tools entirely. `rete serve` turns
any `.rete` into a SPARQL 1.1 Protocol endpoint:

```sh
rete serve my-graph.rete       # → http://127.0.0.1:7878/sparql (--bind to move it)
```

which is the shape LangChain's `RdfGraph`, LlamaIndex, and anything else with a
`query_endpoint` parameter expects. Every published dataset is also an endpoint at
`https://katospiegel-rete.hf.space/sparql/<key>` — details in
[Triple-store interop](interop.md).

The cost of this route is that the model no longer sees the card, schema, and
examples unless you feed them in yourself, which is most of what makes the
in-process loop work.

## Gotchas

Every one of these was hit while writing this page.

**Trim the schema, always.** `schema()` on `boe` — a small graph — is 27
classes and 1,079 relations, **138 KB of JSON**. Passed to a model unfiltered
it swamps the context and buries the classes that matter. Sorting by count and
keeping the top 40/60 brings it to **12 KB** without losing anything the model
queries.

**Return query errors as data, not exceptions.** A bad query raises
`ValueError` with the parser's message (`parse error: error at 1:26: expected
one of LATERAL, SERVICE, [_]`). Catching it and returning
`{"error": "..."}` turns a dead run into a self-repairing one — the model reads
the message and fixes its query. Let it raise and the framework aborts the
step.

**Open the graph once.** The tile cache lives on the handle, so consecutive
queries get cheaper. Constructing `ReteGraphTools` inside a tool function
re-pays the open (10–15 requests) on every call.

**`text_search` returns `[]` unless the file was built with `--text-index`** —
it does not raise, so a search-first tool silently finds nothing. Try
`prefix_search` (label autocomplete, always available) first, and fall back.

**Bound methods work as tools in both frameworks**, and their docstrings become
the tool descriptions the model reads. Write them for the model, not for you.

**`MCPToolset` needs the extra**: `pydantic-ai-slim` alone raises
`ImportError: Please install the fastmcp client…`. Install
`pydantic-ai-slim[mcp]`. And call it through the agent — `async with agent:` —
rather than `toolset.get_tools()`, which requires a `RunContext`.

**Watch `stats()` while developing.** It reports bytes and requests actually
fetched, so an accidentally expensive tool (a `SELECT` with no `LIMIT`, a
schema call on every turn) shows up as a number rather than as a slow agent.

## See also

- [Agentic interfaces](agents.md) — the MCP server, the desktop extension, the
  Claude Code plugin, and the skills.
- [Python API](python.md) — everything `rete-graph` exposes.
- [SPARQL support](sparql.md) — what the engine implements, including OWL 2 QL
  reasoning (`reason=True`) and `SERVICE` federation.
- [Ask the graph](ask-the-graph.md) — the same loop, running entirely in a
  browser tab.
