"""The page at ``/``: what this server is and how to connect to it."""
from __future__ import annotations

import html
from typing import Any, Dict, List


def landing_html(base: str, version: str, datasets: List[Dict[str, Any]], token: bool,
                 author: bool, ask: bool) -> str:
    e = html.escape
    rows = "".join(
        f"<tr><td><code>{e(d['key'])}</code></td><td>{e(str(d.get('label') or ''))}</td>"
        f"<td class=n>{e(str(d.get('triples') or ''))}</td></tr>"
        for d in datasets[:400]
    )
    more = f"<p class=muted>…and {len(datasets) - 400} more at <code>/api/datasets</code>.</p>" if len(datasets) > 400 else ""
    header = ' --header "Authorization: Bearer $RETE_TOKEN"' if token else ""
    auth_note = ("<p>This server needs a token: send <code>Authorization: Bearer &lt;token&gt;</code> "
                 "with every request.</p>") if token else ""
    extras = []
    if author:
        extras.append("authoring tools (build a <code>.rete</code> from RDF, causal diagrams)")
    if ask:
        extras.append("natural-language questions at <code>POST /api/ask</code>")
    extra = f"<p>Also enabled: {', '.join(extras)}.</p>" if extras else ""
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>rete-server</title>
<style>
  :root {{ --fg:#17211d; --muted:#5d6b65; --bg:#f6f8f7; --panel:#fff; --line:#d9e2de; --accent:#0b4f42; }}
  @media (prefers-color-scheme: dark) {{
    :root {{ --fg:#e4ebe8; --muted:#9aa8a2; --bg:#121816; --panel:#1a2320; --line:#2c3833; --accent:#7fd1bd; }}
  }}
  body {{ margin:0; background:var(--bg); color:var(--fg); line-height:1.5;
    font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif; }}
  main {{ max-width:860px; margin:0 auto; padding:40px 16px 64px; }}
  h1 {{ margin:0 0 4px; font-size:1.9rem; }}
  h2 {{ margin:32px 0 8px; font-size:1.15rem; }}
  .muted {{ color:var(--muted); }}
  code, pre {{ font-family:"Cascadia Mono",Consolas,ui-monospace,monospace; font-size:.88em; }}
  pre {{ background:var(--panel); border:1px solid var(--line); border-radius:8px; padding:12px;
    overflow-x:auto; }}
  table {{ width:100%; border-collapse:collapse; background:var(--panel); border:1px solid var(--line); }}
  td {{ padding:6px 10px; border-top:1px solid var(--line); vertical-align:top; font-size:.92rem; }}
  td.n {{ text-align:right; white-space:nowrap; color:var(--muted); }}
  .wrap {{ overflow-x:auto; }}
  a {{ color:var(--accent); }}
  ul {{ padding-left:20px; }}
</style>
</head>
<body>
<main>
  <h1>rete-server</h1>
  <p class="muted">v{e(version)} · {len(datasets)} datasets</p>
  <p>SPARQL, SHACL and MCP over <code>.rete</code> graph files, read lazily by HTTP range.</p>
  {auth_note}
  <h2>Endpoints</h2>
  <ul>
    <li><code>/mcp</code> — MCP server (streamable HTTP) for Claude, ChatGPT and other agents, with an
      interactive graph view that clients rendering MCP Apps (Claude does) show in the conversation</li>
    <li><code>/sparql/&lt;dataset&gt;</code> — a standard SPARQL 1.1 Protocol endpoint per dataset</li>
    <li><code>/api/…</code> — REST: datasets, cards, schema, examples, query, SHACL (<a href="/docs">API docs</a>)</li>
    <li><code>/health</code> — liveness</li>
  </ul>
  {extra}
  <h2>Connect an agent</h2>
  <pre>claude mcp add --transport http rete {e(base)}/mcp{e(header)}</pre>
  <h2>Query</h2>
  <pre>curl -s {e(base)}/sparql/&lt;dataset&gt; --data-urlencode 'query=SELECT * WHERE {{ ?s ?p ?o }} LIMIT 3'{e(header)}</pre>
  <h2>Datasets</h2>
  <div class="wrap"><table>{rows}</table></div>
  {more}
</main>
</body>
</html>"""
