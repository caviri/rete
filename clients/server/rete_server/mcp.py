"""MCP server: the same surface as /api, as tools for LLM apps.

Served over streamable HTTP at ``/mcp/`` (stateless, so any request can be
answered without a session). ``search`` and ``fetch`` follow the ChatGPT
connector contract.

A .rete file describes itself (card, schema and example queries travel inside
it), so the tools take an agent from "what is here?" to a correct query with
no documentation outside the server.
"""
from __future__ import annotations

import json
from datetime import timedelta
from typing import Any, Dict, List, Optional, Union

import anyio

from fastmcp import FastMCP
from fastmcp.exceptions import ToolError
from fastmcp.server.dependencies import Progress
from fastmcp.tools import ToolResult
from mcp_types import ImageContent, TextContent
from fastmcp_tasks import TasksExtension

from . import media, state
from .longrun import (TASK_CONFIG, lane, register_results, run_blocking, shape_sparql,
                      tool_errors)
from .settings import Settings
from .skillset import register_skills
from .views import register_views

READ_ONLY = {"readOnlyHint": True, "openWorldHint": False}
READ_ONLY_OPEN = {"readOnlyHint": True, "openWorldHint": True}

_INSTRUCTIONS = """\
This server queries `.rete` files: single-file RDF knowledge graphs read
lazily by byte range (only the bytes a query touches are fetched).

Recommended workflow:
1. `list_datasets` — what graphs exist and what each is about.
2. `dataset_card(dataset)` — the graph's own metadata: description, license,
   provenance, identity (creators, publisher, DOI when curated), counts.
3. `dataset_schema(dataset)` — classes with instance counts and
   subject-class/predicate/object-class relations. Use these IRIs EXACTLY
   (never invent namespaces; write rdf:type as `a` or in full).
4. `example_queries(dataset)` — runnable SPARQL the dataset ships with; the
   fastest route to correct query shapes.
5. `sparql_query(dataset, query)` — SELECT / ASK / CONSTRUCT / DESCRIBE.
   Always add a LIMIT while exploring. reason=true adds OWL 2 QL entailment.
   A big answer comes back as its first rows and a `result` handle: page the
   rest with `result_rows`. CONTAINS / STRSTARTS / STRENDS / REGEX need strings: wrap an IRI or a
   number in STR(). A FILTER that hits such a type error is false, so rows
   vanish without an error; the result's `warnings` then say which function,
   which argument and what to change. Read them before concluding the data
   has no match. `validate_query` checks a query (and its IRIs against the
   graph) before you run it.
6. `find_entities(dataset, text)` — resolve a name to entity IRIs first when
   a question mentions a specific thing; `describe_entity` shows one.
7. `validate_shacl(dataset, shapes)` — SHACL Core data-quality checks;
   `shacl_shapes(dataset)` lists curated shapes that run as-is.
8. `graph_view(dataset, query | iri | schema=true)` — show results as an
   interactive graph the user explores in the conversation (clients that
   render MCP Apps, as Claude does). Keep it to a few hundred nodes (a
   LIMIT); `graph_view_update` changes the open view and `graph_view_state`
   says what the user selected.
"""

_MEDIA = """\
9. `media_preview(url)` — look at a picture the graph points to: an IIIF
   manifest (`canvas` picks a page), an IIIF image or a plain image URL, back
   as one small JPEG. `graph_view` with view "cards" shows many to the user.
"""

_AUTHORING = """\
Authoring — create graphs, not only read them: `suggest_vocabulary` (find
   existing terms on LOV before minting IRIs) → draft an ontology in Turtle →
   `check_ontology` until clean → `build_rete` (ontology + instances + card +
   examples) → the returned dataset key works at once in sparql_query.
   `causal_diagram` turns claims from a conversation into a diagram and a
   queryable graph.
"""

_FOOTER = """
Skills (clients that support Skills over MCP list them): `query`, `build-rete`,
`network-analysis` and `web-pages` hold the longer guides. The workflow above
is enough without them.

Every result carries `stats` (bytes and requests actually fetched). Each
dataset is also a SPARQL 1.1 Protocol endpoint at `/sparql/<key>` for non-MCP
clients and federated SERVICE clauses.
"""


_tool_errors = tool_errors  # service errors become tool errors (sync or async tools)


def build_mcp(author: bool, settings=None) -> FastMCP:
    settings = settings or state.settings or Settings()
    mcp = FastMCP(name="rete", instructions=(
        _INSTRUCTIONS + (_MEDIA if settings.media_preview else "") + (_AUTHORING if author else "") + _FOOTER))
    # Background tasks (io.modelcontextprotocol/tasks): in-process, so one worker
    # pool per server. Only clients that declare the extension ever get a task.
    mcp.add_extension(TasksExtension(
        url="memory://", concurrency=settings.task_workers,
        execution_ttl=timedelta(seconds=max(3600.0, settings.task_timeout_s * 2))))

    def tool(fn=None, *, read_only: bool = True, task=None, annotations=None):
        def register(f):
            extra = {} if task is None else {"task": task}
            return mcp.tool(_tool_errors(f), annotations=annotations or (READ_ONLY if read_only else None), **extra)
        return register(fn) if fn else register

    @tool
    def list_datasets() -> List[Dict[str, Any]]:
        """Every knowledge graph on this server: key, label, description, size,
        license, and the range-readable URL (usable in any rete client)."""
        keep = ("key", "label", "description", "url", "shards", "triples", "size",
                "size_bytes", "license", "source", "tags", "kind")
        return [{k: d[k] for k in keep if d.get(k) is not None} for d in state.catalog.entries()]

    @tool
    def dataset_card(dataset: str) -> Dict[str, Any]:
        """The Dataset Card embedded in the .rete file: title, description,
        license, provenance and, when curated, identity fields (version,
        creators as ORCID IRIs, publisher as a ROR IRI, DOI, citation), plus
        counts and often example queries. Reads only the card's byte range."""
        card = state.service.dataset_card(dataset)
        return card or {"note": f"dataset {dataset!r} carries no embedded card"}

    @tool
    def dataset_schema(dataset: str) -> Dict[str, Any]:
        """Classes with instance counts and (subject class, predicate, object
        class, count) relations, plus header info and named graphs. These IRIs
        are the exact vocabulary to use in SPARQL — copy them verbatim."""
        return state.service.dataset_schema(dataset)

    @tool
    def example_queries(dataset: str) -> List[Dict[str, Any]]:
        """Runnable example SPARQL for a dataset (the file's own card, else the
        catalog). Each `sparql` runs as-is via sparql_query."""
        return state.service.dataset_examples(dataset)

    @tool(task=TASK_CONFIG)
    async def sparql_query(dataset: Optional[str] = None, query: str = "", reason: bool = False,
                           url: Optional[str] = None, limit: Optional[int] = None,
                           quoted_triple_syntax: str = "rdf12",
                           progress: Progress = Progress()) -> Dict[str, Any]:
        """Run SPARQL 1.1 (SELECT / ASK / CONSTRUCT / DESCRIBE) against a dataset
        key from list_datasets, or a public .rete URL via `url`. Use LIMIT while
        exploring; `reason=true` adds OWL 2 QL entailment. SELECT returns `table`
        (plain values) and W3C-shaped `results`; CONSTRUCT/DESCRIBE return
        `triples`, quoted triples as RDF 1.2 `<<( s p o )>>`
        (`quoted_triple_syntax="rdf-star"` for `<<s p o>>`). `warnings` lists
        FILTER type errors (e.g. CONTAINS on an IRI — wrap it in STR()) that made
        rows silently drop; it is empty when there are none. A big answer (over
        200 rows) comes back as a summary: `columns`, `total_rows`, the first
        rows, `truncated: true` and a `result` handle (and a resource_link);
        read the rest with `result_rows`, or aggregate / LIMIT to keep it
        small. In clients that support MCP tasks a slow query runs as a
        background task with a longer time limit."""
        doc = await run_blocking(
            state.service.run_query, dataset, url, query, reason, limit, quoted_triple_syntax,
            lane=lane(), progress=progress, label=f"querying {dataset or url}")
        return shape_sparql(doc)

    @tool
    def validate_query(query: str, dataset: Optional[str] = None,
                       url: Optional[str] = None, ontology: Optional[str] = None) -> Dict[str, Any]:
        """Check a SPARQL query deterministically before running it: engine
        parse, form and variables, declared vs undeclared prefixes, features,
        unsupported constructs, hygiene. With a `dataset` (or `url`), every
        class/predicate IRI is probed against the graph with a cheap ASK — an
        IRI that matches nothing is a wrong namespace or a typo, the classic
        zero-rows bug. With an `ontology` (Turtle): declarations and subclass
        counts. Fix what it reports, then run sparql_query."""
        from . import qlint

        return qlint.validate_query(query, dataset, url, ontology)

    @tool
    def find_entities(dataset: str, text: str, limit: int = 20) -> List[Dict[str, Any]]:
        """Resolve a name to entity IRIs in one dataset: label prefix search,
        plus full-text search when the file carries a text index. Use the
        returned `subject` IRIs directly in SPARQL."""
        return state.service.entity_search(text, dataset, min(max(limit, 1), 100))

    @tool
    def describe_entity(dataset: str, iri: str) -> Dict[str, Any]:
        """Everything the graph says about one entity IRI (DESCRIBE), as
        N-Triples tokens."""
        return state.service.describe_entity(dataset, iri)

    @tool(task=TASK_CONFIG)
    async def validate_shacl(dataset: Optional[str] = None, shapes: str = "",
                             url: Optional[str] = None, graph: Optional[str] = None,
                             progress: Progress = Progress()) -> Dict[str, Any]:
        """Validate a dataset against SHACL Core shapes (Turtle): cardinalities,
        datatypes, required properties. Only the shapes' targets are fetched.
        Returns {conforms, results} plus fetch stats. shacl_shapes lists curated
        shapes that are known to run."""
        return await run_blocking(
            state.service.shacl_validate, dataset, url, shapes, graph, "json",
            lane=lane(), progress=progress, label=f"validating {dataset or url}")

    @tool
    def shacl_shapes(dataset: str) -> List[Dict[str, Any]]:
        """Curated SHACL shapes for a dataset (title, tip, Turtle `shape`); each
        validates as-is via validate_shacl."""
        return state.service.shacl_shapes(dataset)

    @tool
    def search(query: str) -> Dict[str, List[Dict[str, str]]]:
        """Search this server's graphs. Matches dataset descriptions; a query of
        the form `<dataset-key> <words…>` also searches entities inside that
        dataset. Returns ids for fetch."""
        results: List[Dict[str, str]] = []
        words = query.lower().split()
        for d in state.catalog.entries():
            hay = " ".join(str(d.get(k) or "") for k in ("key", "label", "description", "tags")).lower()
            if words and all(w in hay for w in words):
                results.append({"id": f"dataset::{d['key']}",
                                "title": f"{d.get('label') or d['key']} — {d.get('triples') or '?'} statements",
                                "url": d.get("url") or ""})
        parts = query.split()
        if len(parts) > 1:
            entry = state.catalog.get(parts[0])
            if entry and not entry.get("shards"):
                for hit in state.service.entity_search(" ".join(parts[1:]), parts[0], 10):
                    results.append({"id": f"entity::{parts[0]}::{hit['subject']}",
                                    "title": hit.get("label") or hit["subject"], "url": ""})
        return {"results": results[:25]}

    @tool
    def fetch(id: str) -> Dict[str, Any]:
        """One search result in full: `dataset::<key>` → card and examples;
        `entity::<key>::<iri>` → everything the graph says about that entity."""
        if id.startswith("dataset::"):
            key = id[len("dataset::"):]
            entry = state.catalog.get(key) or {}
            sharded = bool(entry.get("shards"))
            body = {"catalog": state.public_entry(entry),
                    "card": None if sharded else state.service.dataset_card(key),
                    "examples": [] if sharded else state.service.dataset_examples(key)}
            return {"id": id, "title": entry.get("label") or key, "text": json.dumps(body, indent=2),
                    "url": entry.get("url") or "", "metadata": {"kind": "dataset"}}
        if id.startswith("entity::"):
            _, key, iri = id.split("::", 2)
            doc = state.service.describe_entity(key, iri)
            return {"id": id, "title": iri, "text": json.dumps(doc.get("triples") or [], indent=2),
                    "url": "", "metadata": {"kind": "entity", "dataset": key}}
        raise ToolError(f"unknown id shape {id!r} (expected dataset::… or entity::…)")

    if settings.media_preview:
        @tool(annotations=READ_ONLY_OPEN)
        async def media_preview(url: str, canvas: Union[int, str] = 0, max_px: int = 512) -> ToolResult:
            """Look at a picture the graph points to. `url` is an IIIF manifest
            (v2 or v3), an IIIF image service or info.json, or an image URL
            (jpeg, png, webp, gif, tiff). For a manifest, `canvas` is the page:
            a 0-based index or a canvas id. Returns one JPEG (longest side
            `max_px`, 128 to 1024) and a JSON note with the label, canvas,
            number of canvases, size and the image URL used. Only public hosts
            are fetched; the image is shrunk server-side."""
            try:
                with anyio.fail_after(media.TOTAL_TIMEOUT_S + 3):
                    jpeg, facts = await run_blocking(
                        media.preview, url, canvas, max_px, settings.media_allow_private)
            except TimeoutError:
                raise ToolError("timed out fetching the picture") from None
            except media.MediaError as e:
                raise ToolError(str(e)) from None
            return ToolResult(
                content=[ImageContent(type="image", data=media.b64(jpeg), mimeType="image/jpeg"),
                         TextContent(type="text", text=json.dumps(facts))],
                structured_content=facts)

    if author:
        @tool
        def suggest_vocabulary(query: str, limit: int = 8) -> List[Dict[str, Any]]:
            """Search Linked Open Vocabularies (lov.linkeddata.es) for EXISTING
            terms matching some concept words. Check here before minting IRIs —
            reuse or rdfs:subClassOf what exists."""
            from . import author as authoring

            return authoring.suggest_vocabulary(query, limit)

        @tool
        def check_ontology(ontology: str, format: str = "ttl") -> Dict[str, Any]:
            """Validate an ontology draft: strict parse, a profile, a lint battery
            (domains/ranges over undeclared classes, dangling subClassOf, missing
            labels, properties typed both object and datatype, subclass cycles)
            and an OWL 2 QL smoke test. Iterate until `ok`, THEN build_rete."""
            from . import author as authoring

            return authoring.check_ontology(ontology, format)

        @tool(read_only=False, task=TASK_CONFIG)
        async def build_rete(rdf: str, format: str = "ttl", card: Optional[Dict[str, Any]] = None,
                             examples: Optional[List[Dict[str, str]]] = None, text_index: bool = False,
                             include_base64: bool = False,
                             progress: Progress = Progress()) -> Dict[str, Any]:
            """Build a .rete from RDF text (ontology + instances) on this server.
            Give it a `card` (title, description, license; when known: version,
            creators [{name, orcid}], publisher {name, ror}, doi, cite_as,
            keywords, theme IRIs) and runnable `examples` ({title, question,
            sparql}). The returned `dataset` key works at once in sparql_query,
            validate_shacl and dataset_card. Ephemeral: set include_base64=true
            to hand the user the file itself."""
            from . import author as authoring

            return await run_blocking(authoring.build_rete, rdf, format, card, examples, text_index,
                                      include_base64, progress=progress, label="building the .rete")

        @tool(read_only=False)
        def causal_diagram(claims: List[Dict[str, Any]], title: str = "Causal diagram",
                           render: str = "both", build: bool = True) -> Dict[str, Any]:
            """Causal claims from a conversation → a diagram AND a queryable graph.
            YOU extract the claims, each {cause, effect, relation
            (causes|prevents|enables|correlates), quote, speaker, confidence};
            this returns `mermaid`, `dot`, an `svg_data_uri` (Graphviz) and a
            CauseNet-aligned .rete (cn:cause / cn:effect) whose dataset key works
            in sparql_query at once."""
            from . import author as authoring

            return authoring.causal_diagram(claims, title, render, build)

    # The graph view: its page and tools. A file the user opens in a view
    # becomes a dataset here, as a build_rete file does, so it goes with authoring.
    register_views(mcp, uploads=author)
    register_skills(mcp)
    # Stored answers: result_rows and rete://results/{id}.
    register_results(mcp, READ_ONLY)
    return mcp
