# Tools and integrations

You can use rete through an existing interface as well as a language client.
Choose a tool by the task: browsing a file, writing queries, displaying geometry
or giving an agent access to graph data.

## Browse and query

| Tool | Use it for |
|---|---|
| [Playground](playground-guide.md) | Explore a dataset, run queries and share results |
| [Plaza](plaza-guide.md) | Browse published datasets and their cards |
| [File Explorer](file-explorer.md) | Inspect local or remote graphs through a file-oriented interface |
| [SPARQL IDE](yasgui-guide.md) | Write and compare queries in tabs |
| [Comunica](comunica.md) | Use a `.rete` file as an RDF/JS source |

For an existing triple store or SPARQL client,
[interoperability](interop.md) explains when to export data, use an endpoint
or federate a query.

## Use graphs in an application

[Blender](blender.md) turns query results into scenes. The
[language clients](clients.md) are the starting point for your own application.
See [worked examples](recipes.md) for complete uses of those interfaces.

## Give an agent graph access

[Agent interfaces](agents.md) covers MCP, the plugin and reusable skills.
[Agent frameworks](agent-frameworks.md) shows in-process integrations with
LangChain and Pydantic AI. The [speech annotation example](fallacies.md)
walks through constructing and checking a graph from a conversation.

Distinguish where the graph is queried from where the model runs. A local
graph tool can still send returned context to a hosted language model.
