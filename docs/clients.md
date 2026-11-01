# Language clients

Choose the client for the environment where your application already runs.
Each guide starts with installation and a query, then explains the result
types and the constraints of that runtime.

| Environment | Start here | Next step |
|---|---|---|
| Python scripts and notebooks | [Python](python.md) | [Build a graph in Python](python-build-tutorial.md) |
| Node or a browser application | [JavaScript](javascript.md) | [JS lab](jslab-guide.md), a runnable visualization example |
| R analysis | [R](r.md) | Work with query results as data frames |
| Rust application | [Rust API](rust-api.md) | [API and format reference](reference.md) |
| Java application | [Java client](https://github.com/caviri/rete/tree/main/clients/java) | Choose the byte-array or range-reader API |
| Shell or a pipeline | [Quickstart](getting-started.md) | [CLI reference](cli.md) |

## Try a client without local setup

[JupyterLite](jupyterlite-guide.md) runs Python notebooks in your browser.
The [Playground](playground-guide.md) is a ready-made interface for querying
and exploring datasets.

In browser JavaScript, remote range reads belong in a worker; opening bytes
on the main thread is a different path. Follow the [JavaScript guide](javascript.md)
for application code, or the [WASM bindings reference](browser.md) if you
are embedding the lower-level engine yourself.

## Connect existing tools

For RDF/JS, Blender, agent frameworks or an MCP client, see
[Tools and integrations](integrations.md). Building or releasing a client
belongs in [Client development](clients-dev.md).
