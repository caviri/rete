"""MCP plane: the same service surface as /api, shaped for LLM apps.

FastMCP server exposed over streamable HTTP at /mcp — connect it to
Claude, ChatGPT (developer mode or as a connector: the ``search`` and
``fetch`` tools follow the connector contract), or any MCP client.

The design premise: a .rete file is self-describing (card, schema, example
queries travel inside the file), so the tools give an agent everything it
needs to go from "what datasets exist?" to a correct SPARQL query without
out-of-band documentation.
"""
from __future__ import annotations

import json
from typing import Any, Dict, List, Optional

from fastmcp import FastMCP

import rete_service as svc

INSTRUCTIONS = """\
This server queries `.rete` files — single-file RDF knowledge graphs hosted on
plain HTTP storage and read lazily by byte range (only the bytes a query
touches are fetched; fetched pieces are cached on disk).

Recommended workflow:
1. `list_datasets` — see what graphs exist and what each is about.
2. `dataset_card(dataset)` — the graph's own metadata: description, license,
   provenance, counts.
3. `dataset_schema(dataset)` — classes with instance counts and
   subject-class/predicate/object-class relations. Use the IRIs EXACTLY as
   returned (never invent namespaces; `a`/rdf:type shortcuts require the
   full IRI form `<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>` unless
   a PREFIX is declared).
4. `example_queries(dataset)` — runnable SPARQL the dataset ships with; the
   fastest route to correct query shapes.
5. `sparql_query(dataset, query)` — run SELECT / ASK / CONSTRUCT / DESCRIBE.
   Always add a LIMIT while exploring. Set reason=true for OWL 2 QL
   entailment on ontology-bearing datasets.
6. `find_entities(dataset, text)` — resolve a name to entity IRIs first when
   a question mentions a specific thing.

Every result carries `stats` (bytes/requests actually fetched) — with lazy
range reads a good query touches a tiny fraction of the file.
"""

mcp = FastMCP(name="rete-graphs", instructions=INSTRUCTIONS)


@mcp.tool
def list_datasets() -> List[Dict[str, Any]]:
    """All published .rete knowledge graphs on this server: key, label,
    description, size, license, and the direct range-readable URL (usable in
    any rete client — Python, R, JS, CLI, or the browser playground)."""
    keep = ("key", "label", "description", "url", "shards", "triples", "size", "license", "source", "tags")
    return [{k: d[k] for k in keep if d.get(k) is not None} for d in svc.load_catalog()]


@mcp.tool
def dataset_card(dataset: str) -> Dict[str, Any]:
    """The Dataset Card embedded in the .rete file itself: title, description,
    license, provenance, creation date, counts, and often example queries.
    Lazy — reading it fetches only the card's byte range."""
    card = svc.dataset_card(dataset)
    return card or {"note": f"dataset {dataset!r} carries no embedded card"}


@mcp.tool
def dataset_schema(dataset: str) -> Dict[str, Any]:
    """The graph's class/predicate profile: classes with instance counts and
    (subject class, predicate, object class, count) relations, plus header
    info and named graphs. The IRIs here are the exact vocabulary to use in
    SPARQL — copy them verbatim."""
    return svc.dataset_schema(dataset)


@mcp.tool
def example_queries(dataset: str) -> List[Dict[str, Any]]:
    """Runnable example SPARQL queries for a dataset (from the file's own
    card when present, else the published catalog). Each entry's `sparql`
    runs as-is via sparql_query — the fastest way to learn a graph's shape."""
    return svc.dataset_examples(dataset)


@mcp.tool
def sparql_query(dataset: Optional[str] = None, query: str = "", reason: bool = False,
                 url: Optional[str] = None, limit: Optional[int] = None) -> Dict[str, Any]:
    """Run a SPARQL 1.1 query (SELECT / ASK / CONSTRUCT / DESCRIBE) against a
    dataset key from list_datasets — or any range-readable .rete URL via
    `url`. Reads are lazy and disk-cached, so repeated queries get faster.
    Use LIMIT while exploring; `reason=true` adds OWL 2 QL entailment.
    SELECT returns `table` (plain values) and W3C-shaped `results`."""
    return svc.run_query(dataset, url, query, reason, limit)


@mcp.tool
def find_entities(dataset: str, text: str, limit: int = 20) -> List[Dict[str, Any]]:
    """Resolve a name/word to entity IRIs in one dataset, via label-prefix
    search plus full-text search when the file carries a text index. Use the
    returned `subject` IRIs directly in SPARQL."""
    return svc.entity_search(text, dataset, min(limit, 100))


@mcp.tool
def describe_entity(dataset: str, iri: str) -> Dict[str, Any]:
    """Everything the graph says about one entity IRI (DESCRIBE): all triples
    with it as subject, in N-Triples token form."""
    return svc.describe_entity(dataset, iri)


# --------------------------------------------------------------------------- #
# ChatGPT connector contract: search + fetch
# --------------------------------------------------------------------------- #

@mcp.tool
def search(query: str) -> Dict[str, List[Dict[str, str]]]:
    """Search this server's knowledge graphs. Matches dataset descriptions
    first; terms of the form `<dataset-key> <words…>` additionally search
    entities inside that dataset. Returns result ids usable with fetch."""
    results: List[Dict[str, str]] = []
    words = query.lower().split()
    catalog = svc.load_catalog()
    for d in catalog:
        hay = " ".join(str(d.get(k) or "") for k in ("key", "label", "description", "tags")).lower()
        if all(w in hay for w in words):
            results.append({
                "id": f"dataset::{d['key']}",
                "title": f"{d.get('label') or d['key']} — {d.get('triples') or ''} triples",
                "url": d.get("url") or "",
            })
    if words and len(words) > 1:
        maybe_key = query.split()[0]
        if svc.find_dataset(maybe_key) and not svc.find_dataset(maybe_key).get("shards"):
            text = " ".join(query.split()[1:])
            for hit in svc.entity_search(text, maybe_key, 10):
                results.append({
                    "id": f"entity::{maybe_key}::{hit['subject']}",
                    "title": hit.get("label") or hit["subject"],
                    "url": "",
                })
    return {"results": results[:25]}


@mcp.tool
def fetch(id: str) -> Dict[str, Any]:
    """Fetch one search result in full. `dataset::<key>` returns the card,
    schema and examples; `entity::<key>::<iri>` returns everything the graph
    says about that entity."""
    if id.startswith("dataset::"):
        key = id[len("dataset::"):]
        entry = svc.find_dataset(key) or {}
        body = {
            "catalog": entry,
            "card": None if entry.get("shards") else svc.dataset_card(key),
            "examples": svc.dataset_examples(key) if not entry.get("shards") else [],
        }
        return {"id": id, "title": entry.get("label") or key,
                "text": json.dumps(body, indent=2), "url": entry.get("url") or "",
                "metadata": {"kind": "dataset"}}
    if id.startswith("entity::"):
        _, key, iri = id.split("::", 2)
        doc = svc.describe_entity(key, iri)
        return {"id": id, "title": iri, "text": json.dumps(doc.get("triples") or [], indent=2),
                "url": "", "metadata": {"kind": "entity", "dataset": key}}
    raise ValueError(f"unknown id shape {id!r} (expected dataset::… or entity::…)")
