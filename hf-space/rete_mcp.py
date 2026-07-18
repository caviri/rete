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
7. `validate_shacl(dataset, shapes)` — data-quality checks: validate SHACL
   Core shapes (Turtle) against the graph; `shacl_shapes(dataset)` lists
   curated example shapes that run as-is.
8. AUTHORING loop — create knowledge graphs, don't just read them:
   `suggest_vocabulary` (find existing terms on LOV before minting IRIs) →
   draft an ontology in Turtle → `check_ontology` until clean →
   `build_rete` (ontology + instances + card + examples) → the returned
   dataset key works immediately in sparql_query / validate_shacl.

Every result carries `stats` (bytes/requests actually fetched) — with lazy
range reads a good query touches a tiny fraction of the file. Each dataset
is also a standard SPARQL 1.1 Protocol endpoint at `/sparql/<key>` — and
`/sparql/<any-full-.rete-URL>` serves ANY published .rete file the same
way — for non-MCP clients and federated SERVICE clauses.
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


@mcp.tool
def validate_shacl(dataset: Optional[str] = None, shapes: str = "",
                   url: Optional[str] = None, graph: Optional[str] = None) -> Dict[str, Any]:
    """Validate a dataset against SHACL Core shapes written in Turtle —
    data-quality and integrity checks (cardinalities, datatypes, required
    properties…). Lazy: only the shapes' targets are fetched. Returns the
    W3C-style validation report ({conforms, results}) plus fetch stats.
    Use shacl_shapes first for curated, known-good shapes."""
    return svc.shacl_validate(dataset, url, shapes, graph, "json")


@mcp.tool
def shacl_shapes(dataset: str) -> List[Dict[str, Any]]:
    """Curated example SHACL shapes for a dataset (title, tip, and the
    Turtle `shape`). Each shape validates as-is via validate_shacl — the
    fastest way to see what data-quality contracts a graph is meant to
    uphold."""
    return svc.shacl_shapes(dataset)


@mcp.tool
def suggest_vocabulary(query: str, limit: int = 8) -> List[Dict[str, Any]]:
    """Search Linked Open Vocabularies (lov.linkeddata.es) for EXISTING
    ontology terms matching some concept words (e.g. 'argument fallacy
    premise' finds AIF). Always check here before minting new IRIs — reuse
    or rdfs:subClassOf what exists. Returns term IRIs with their vocabulary
    prefix, type, and relevance score."""
    import rete_author
    return rete_author.suggest_vocabulary(query, limit)


@mcp.tool
def check_ontology(ontology: str, format: str = "ttl") -> Dict[str, Any]:
    """Validate an ontology draft (Turtle). Runs: strict parse, a profile
    (class/property counts), a lint battery — domains/ranges over
    undeclared classes, dangling subClassOf targets, missing labels/
    comments, properties typed both object+datatype, subclass cycles —
    and an OWL 2 QL reasoner smoke test. Iterate until `ok` is true and
    the warnings you care about are gone, THEN build with build_rete."""
    import rete_author
    return rete_author.check_ontology(ontology, format)


@mcp.tool
def build_rete(rdf: str, format: str = "ttl",
               card: Optional[Dict[str, Any]] = None,
               examples: Optional[List[Dict[str, str]]] = None,
               text_index: bool = False,
               include_base64: bool = False) -> Dict[str, Any]:
    """Build a real .rete file from RDF text (ontology + instances) and
    serve it at an ephemeral URL. Give it a `card` (title, description,
    license) and runnable `examples` ({title, question, sparql}) so the
    file is self-describing. The returned `dataset` key works immediately
    in sparql_query / validate_shacl / dataset_card — query what you just
    built, in this same conversation. Ephemeral until the next Space
    restart: set include_base64=true to hand the user the file itself."""
    import rete_author
    return rete_author.build_rete(rdf, format, card, examples, text_index, include_base64)


@mcp.tool
def causal_diagram(claims: List[Dict[str, Any]], title: str = "Causal diagram",
                   render: str = "both", build: bool = True) -> Dict[str, Any]:
    """Turn causal claims extracted from a conversation into a diagram AND a
    queryable graph. YOU extract the claims from the transcript — each as
    {cause, effect, relation (causes|prevents|enables|correlates), quote,
    speaker, confidence} — and this tool returns: `mermaid` (render it in
    your answer), `dot`, an `svg_data_uri` (Graphviz layout, embeddable),
    and a served .rete aligned with CauseNet's cn:cause/cn:effect — so the
    returned dataset key works in sparql_query at once, including the
    embedded federated example that checks the conversation's claims
    against CauseNet's 11M web-mined causal relations."""
    import rete_author
    return rete_author.causal_diagram(claims, title, render, build)


@mcp.tool
def embed_media(urls: List[str], max_dimension: int = 1024, webp_quality: int = 80) -> List[Dict[str, Any]]:
    """Fetch a list of media URLs and return each as a base64 `data:` URI,
    ready to inline into generated HTML (img src, download links…). Images
    are recompressed to WebP and downscaled to max_dimension on the long
    side — typically 3-10x smaller — other types pass through verbatim with
    their MIME. Per-entry `ok`/`error` so one bad URL never fails the batch.
    Use small max_dimension (e.g. 512) when embedding many images."""
    import rete_media
    return rete_media.embed_urls(urls, max_dimension, webp_quality)


@mcp.tool
def media_preview(url: str, max_dimension: int = 512, webp_quality: int = 80) -> Dict[str, Any]:
    """One representative image (WebP data URI) for a media URL — perfect
    for thumbnails in generated HTML. Understands: images (recompressed),
    PDFs (first page rendered), videos (a frame, fetched lazily over HTTP
    Range — the file is never downloaded whole), IIIF info.json and
    Presentation manifests v2/v3, and HTML pages (og:image/twitter:image).
    Media URLs typically come from query results — e.g. IIIF manifests,
    scans, and PDFs in the datasets here."""
    import rete_media
    return rete_media.preview(url, max_dimension, webp_quality)


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
