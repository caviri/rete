---
title: rete
emoji: 🕸️
colorFrom: green
colorTo: indigo
sdk: docker
app_port: 7860
pinned: false
license: apache-2.0
short_description: SPARQL, SHACL and MCP over .rete graph files
---

# rete-server

REST, a SPARQL 1.1 Protocol endpoint per dataset, and an MCP server over the
published `.rete` catalog, read lazily from `data.graphplaza.com`.

- `/` — the home page: datasets, a query box, how to connect an agent
- `/mcp` — MCP (streamable HTTP), with the graph view, skills and long queries as tasks
- `/sparql/<dataset>` — SPARQL 1.1 Protocol
- `/api/…` — REST, documented at `/docs`
- `/health`

It is open: no token, no sign-up. Uploads and building datasets are off here;
run your own copy for those (`docker run -p 7860:7860 ghcr.io/caviri/rete-server`).

This Space runs `ghcr.io/caviri/rete-server`; the source is
[`clients/server`](https://github.com/caviri/rete/tree/main/clients/server).
