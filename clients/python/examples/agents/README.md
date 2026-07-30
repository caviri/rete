# Agent frameworks over a `.rete` graph

Runnable companions to the
[LangChain & Pydantic AI tutorial](https://caviri.github.io/rete/agent-frameworks.html).
Each script gives a model five tools over **one** `.rete` file — card, schema,
examples, entity lookup, SPARQL — and lets it answer questions by querying.
The file is read by byte range, local or remote, with no server in between.

| File | What it is |
| --- | --- |
| `rete_tools.py` | The tool layer. Framework-agnostic: five methods returning JSON, plus the system prompt that teaches the loop. |
| `pydantic_ai_agent.py` | [Pydantic AI](https://ai.pydantic.dev) agent — bound methods passed straight to `Agent(tools=[...])`. |
| `langchain_agent.py` | [LangChain](https://python.langchain.com) agent — the same methods via `StructuredTool.from_function` and `create_agent`. |

## Run

```sh
pip install rete-graph "pydantic-ai-slim[anthropic]"     # ... or
pip install rete-graph langchain langchain-anthropic

export ANTHROPIC_API_KEY=...
python pydantic_ai_agent.py "How many laws are here? Show me three with their titles."
python langchain_agent.py   "Which three laws are the oldest?"
```

Both default to the published Spanish-legislation graph
(`https://data.graphplaza.com/boe/boe.rete`, 7 MB on a CDN). Point them
anywhere — including a private file that never leaves the machine:

```sh
RETE_GRAPH=/path/to/my-graph.rete python langchain_agent.py "What is in this graph?"
```

Any OpenAI-compatible endpoint works instead of Anthropic (`pip install
langchain-openai` for the LangChain script):

```sh
export OPENAI_BASE_URL=https://your-endpoint/v1 OPENAI_API_KEY=... MODEL=...
```

Each run prints the bytes and requests actually fetched, so the cost of the
agent's exploration stays visible:

```
[graph reads: {'fileLength': 6958628, 'bytes': 3065519, 'requests': 18}]
```

## Verified

Both scripts were run end to end against a live tool-calling model and the
remote `boe` graph: the agent reads the schema, copies the ELI IRIs out of it,
writes its own SPARQL, and answers from the rows. The MCP alternative —
pointing either framework at the hosted rete MCP server instead — is in the
same tutorial.
