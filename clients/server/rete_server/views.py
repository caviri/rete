"""The graph view: an MCP App that the client draws in the conversation.

This server renders nothing. It hands the client the page
(``ui://rete/graph-view.html``, the same page the Claude Desktop extension
ships, built from ``clients/mcpb/src/view``); the client shows it in a
sandboxed frame, and the page calls this server's app-only tools for its data.
A client without MCP Apps just gets the tools' text answers.

A port of the extension's ``src/views.mjs`` and ``src/view-model.mjs``: the
same tools, arguments and answers, so one page works against both. Change them
together.

The data never travels through the model: ``graph_view`` answers with a view id
and counts, and the page fetches the elements with app-only tools (visibility
``["app"]``: the host keeps them from the model). MCP Apps has no server push,
so a change Claude makes (``graph_view_update``) is kept as a numbered patch,
which the page collects on its next sync, about every second while it is
shown, together with what the user selected, which ``graph_view_state``
reports back to Claude.

Views live in this process's memory, the newest ``MAX_VIEWS`` of them, each
under an id nobody can guess: anyone with a token reaches the same tools, so
the id is what keeps one person's view from another's.
"""
from __future__ import annotations

import base64
import binascii
import functools
import json
import os
import re
import secrets
import threading
import time
import typing
from collections import OrderedDict
from pathlib import Path
from typing import Any, Callable, Dict, List, Literal, Optional, Tuple

import rete_graph as rete
from fastmcp import FastMCP
from fastmcp.apps import AppConfig, ResourceCSP
from fastmcp.exceptions import ToolError
from fastmcp.tools.base import ToolResult
from mcp.types import TextContent
from pydantic import BaseModel, Field

from . import state
from .engine import SANDBOX, Busy, QueryTimeout, RequestError

VIEW_URI = "ui://rete/graph-view.html"
PAGE = Path(__file__).resolve().parent / "view" / "graph-view.html"

# What the page may load for the card view (MCP Apps: _meta.ui.csp): pictures, audio and video
# (resourceDomains) and the file of a 3D model (connectDomains). A host honours exactly the origins
# declared, so these are the sites the published graphs point at. RETE_VIEW_MEDIA_DOMAINS replaces
# the list (comma separated; origins as in a CSP, https://*.example.org for subdomains, https: for
# any https site; "none" for no network at all). Same list and rules as views.mjs.
MEDIA_DOMAINS = [
    "https://*.wikimedia.org",
    "https://*.graphplaza.com",
    "https://huggingface.co",
    "https://*.huggingface.co",
    "https://*.hf.co",
    "https://archive.org",
    "https://*.archive.org",
    "https://*.githubusercontent.com",
]
_MEDIA_ORIGIN = re.compile(r"(https?://[\w.*-]+(:\d+)?|https?:)", re.ASCII)


def media_domains(env: Optional[Dict[str, str]] = None) -> List[str]:
    given = (os.environ if env is None else env).get("RETE_VIEW_MEDIA_DOMAINS")
    if given is None or given.strip() == "":
        return list(MEDIA_DOMAINS)
    if given.strip().lower() == "none":
        return []
    return [d for d in (x.strip() for x in given.split(",")) if _MEDIA_ORIGIN.fullmatch(d)]


# The card view's column types and the hive layout's choices: copies of FIELD_TYPE_NAMES
# (clients/mcpb/src/view/cards.js) and HIVE_AXES / HIVE_ORDERS (view/hive.js). Change together.
FieldType = Literal["auto", "text", "link", "button", "image", "iiif", "geo", "audio", "video", "model3d",
                    "pdf", "markdown", "layer", "number"]
HiveAxes = Literal["auto", "type", "role", "kind", "degree"]
HiveOrder = Literal["degree", "in", "out", "name"]
FIELD_TYPE_NAMES = typing.get_args(FieldType)

# The tabs a query's hint may name (clients/chrome/lib/hints.js VIEWS).
_HINT_VIEWS = {"table", "cards", "graph", "map"}
_HINT_VIEW = re.compile(r"view\s*:\s*([a-z]+)\s*", re.I | re.ASCII)


def hinted_view(sparql: Optional[str]) -> Optional[str]:
    """The tab a query's comment lines name (``# view: map``): RetelensHints.parse(text).view.
    Only comments that start a line count (a # inside an IRI or a string is no comment); the
    last valid one wins."""
    view = None
    for line in re.split(r"\r?\n", str(sparql or "")):
        m = re.match(r"\s*#\s?(.*)$", line)
        if m and (h := _HINT_VIEW.fullmatch(m.group(1).strip())) and h.group(1).lower() in _HINT_VIEWS:
            view = h.group(1).lower()
    return view


def with_hinted_view(options: Dict[str, Any], text: Optional[str]) -> Dict[str, Any]:
    """A query's ``# view:`` hint picks the tab when the options name none."""
    if options.get("view") or not text:
        return options
    view = hinted_view(text)
    return {**options, "view": view} if view else options


MAX_VIEWS = 200
MAX_NODES = 1500  # per view; more is unreadable on a canvas anyway
MAX_EDGES = 4000
MAX_ROWS = 5000  # table rows a view keeps
MAX_LOG = 300  # patches kept for views that are behind
NEIGHBOURS = 150  # statements per direction when a node is expanded
LABEL_BATCH = 200
SCHEMA_CLASSES = 60  # classes a schema view draws: the most frequent
# What a file opened in a view shows first, unless something else was asked
# for. A SELECT, not a CONSTRUCT: the engine does not apply LIMIT to a
# CONSTRUCT, and s/p/o columns read as triples all the same.
SAMPLE = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 300"
# A file being opened in a view is held whole until its last part: one the
# view stopped sending is dropped after this long.
UPLOAD_IDLE_S = 120
TEXT_MAX_BYTES = 32 << 20  # RDF text is built here, in memory

RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
LABELS = [
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2004/02/skos/core#prefLabel",
    "http://schema.org/name",
    "https://schema.org/name",
    "http://xmlns.com/foaf/0.1/name",
    "http://purl.org/dc/terms/title",
    "http://purl.org/dc/elements/1.1/title",
]
RDF_FORMATS = {
    ".nt": "nt", ".ntriples": "nt", ".nq": "nq", ".nquads": "nq", ".ttl": "ttl",
    ".turtle": "ttl", ".trig": "trig", ".rdf": "rdfxml", ".owl": "rdfxml", ".xml": "rdfxml",
    ".jsonld": "jsonld", ".yamlld": "yamlld", ".jelly": "jelly",
}


def _format_of(name: str) -> Optional[str]:
    """The RDF syntax a file name's extension says, or None (a .rete)."""
    suffix = Path(str(name)).suffix.lower()
    return RDF_FORMATS.get(suffix)


def _mb(n: int) -> str:
    return f"{n / (1 << 20):.{1 if n < 10 << 20 else 0}f} MB"


# --------------------------------------------------------------------------- #
# The model: what a view holds (view-model.mjs)
# --------------------------------------------------------------------------- #

def look_of(options: Dict[str, Any], current: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
    """A diagram's own look, unless the call sets it: boxes and classes read
    best laid out in layers with square connectors, dots in a force layout."""
    look = {k: v for k, v in (options or {}).items() if v is not None}
    if look.get("diagram") and look["diagram"] != (current or {}).get("diagram", "graph"):
        boxes = look["diagram"] != "graph"
        look.setdefault("layout", "layered" if boxes else "cose")
        look.setdefault("edge_style", "square" if boxes else "curved")
    return look


class View:
    """A view's graph, kept here so a view that reloads, or falls behind,
    gets it whole."""

    def __init__(self, id: str, dataset: Optional[str], title: Optional[str], options: Dict[str, Any]):
        self.id = id
        self.dataset = dataset
        self.title = title
        self.options = dict(options)
        self.nodes: Dict[str, Dict[str, Any]] = {}  # token -> node
        self.edges: Dict[str, Dict[str, Any]] = {}  # id -> edge
        self.table: Optional[Dict[str, Any]] = None  # {vars, rows, terms, query} of the last SELECT
        self.aggregates: Dict[str, Dict[str, Any]] = {}  # id -> {id, label, members}
        self.aggregated = 0  # aggregates made, for their ids
        self.version = 0
        self.log: List[Dict[str, Any]] = []
        self.ui: Optional[Dict[str, Any]] = None  # what the page last reported
        self.seen_at: Optional[float] = None  # when the page last synced
        self.status = ""
        self.awaiting: Optional[Dict[str, Any]] = None  # {file}: a file the view asks the user for
        self.then: Optional[Dict[str, Any]] = None  # what to show once that file is open
        self.touched_at = time.time()
        # The page syncs while Claude changes the view: one at a time.
        self.lock = threading.RLock()

    def patch(self, change: Dict[str, Any]) -> int:
        """Record a change: it goes into the graph here and, numbered, to the page."""
        self.version += 1
        self.log.append({"version": self.version, **change})
        if len(self.log) > MAX_LOG:
            del self.log[: len(self.log) - MAX_LOG]
        self.touched_at = time.time()
        return self.version

    def since(self, since: Optional[int]) -> Dict[str, Any]:
        """What a page at ``since`` lacks: the patches after it, or everything
        when it is too far behind."""
        if not since or not self.log or since < self.log[0]["version"] - 1:
            return {"version": self.version, "snapshot": self.snapshot()}
        return {"version": self.version, "patches": [p for p in self.log if p["version"] > since]}

    def snapshot(self) -> Dict[str, Any]:
        return {
            "view_id": self.id,
            "dataset": self.dataset,
            "title": self.title,
            "options": self.options,
            "nodes": list(self.nodes.values()),
            "edges": list(self.edges.values()),
            "table": self.table,
            "status": self.status,
            "awaiting": self.awaiting,
            "aggregates": list(self.aggregates.values()),
        }

    def add(self, graph: Dict[str, List[Dict[str, Any]]]) -> Dict[str, List[Dict[str, Any]]]:
        """Merge elements; the ones actually new go in the patch."""
        nodes, edges = [], []
        for n in graph["nodes"]:
            had = self.nodes.get(n["id"])
            if had is not None:
                # A node seen again may bring its label, types or more properties.
                changed = False
                if not had["label"] and n["label"]:
                    had["label"] = n["label"]
                    changed = True
                for t in n["types"]:
                    if t not in had["types"]:
                        had["types"].append(t)
                        changed = True
                for p in n["props"]:
                    if not any(a == p[0] and b == p[1] for a, b in had["props"]):
                        had["props"].append(p)
                        changed = True
                if changed:
                    nodes.append(had)
            elif len(self.nodes) < MAX_NODES:
                self.nodes[n["id"]] = n
                nodes.append(n)
        for e in graph["edges"]:
            if e["id"] in self.edges or e["source"] not in self.nodes or e["target"] not in self.nodes:
                continue
            if len(self.edges) >= MAX_EDGES:
                break
            self.edges[e["id"]] = e
            edges.append(e)
        return {"nodes": nodes, "edges": edges}

    def clear(self) -> None:
        self.nodes.clear()
        self.edges.clear()
        self.table = None
        self.aggregates.clear()

    def remove(self, ids: List[str]) -> int:
        """Take nodes out, with their edges; a node leaves its aggregate, and an
        emptied one goes."""
        gone = {i for i in ids if i in self.nodes}
        for i in gone:
            del self.nodes[i]
        for eid in [eid for eid, e in self.edges.items() if e["source"] in gone or e["target"] in gone]:
            del self.edges[eid]
        for a in list(self.aggregates.values()):
            a["members"] = [m for m in a["members"] if m not in gone]
            if not a["members"]:
                del self.aggregates[a["id"]]
        self.patch({"op": "remove", "ids": list(gone)})
        return len(gone)

    def aggregate(self, ids: List[str], label: Optional[str] = None) -> Dict[str, Any]:
        """Draw nodes as one. An aggregate among them is merged into the new
        one: a node belongs to one aggregate at a time."""
        members: Dict[str, None] = {}  # ordered set
        merged: List[str] = []
        for i in ids:
            a = self.aggregates.get(i)
            if a is not None:
                for m in a["members"]:
                    members[m] = None
                merged.append(a["id"])
            elif i in self.nodes:
                members[i] = None
        if len(members) < 2:
            raise ToolError("aggregate at least two nodes of the view")
        shrunk = []
        for a in self.aggregates.values():
            if a["id"] in merged or not any(m in members for m in a["members"]):
                continue
            a["members"] = [m for m in a["members"] if m not in members]
            if a["members"]:
                shrunk.append(a)
            else:
                merged.append(a["id"])
        for i in merged:
            self.aggregates.pop(i, None)
        if merged:
            self.patch({"op": "ungroup", "ids": merged})
        # Copies: a patch says what was, and the aggregate changes on.
        for a in shrunk:
            self.patch({"op": "aggregate", "aggregate": {**a, "members": list(a["members"])}})
        self.aggregated += 1
        agg = {"id": f"agg:{self.aggregated}", "label": label or f"{len(members)} nodes", "members": list(members)}
        self.aggregates[agg["id"]] = agg
        self.patch({"op": "aggregate", "aggregate": {**agg, "members": list(agg["members"])}})
        return agg

    def ungroup(self, id: str) -> int:
        """Split aggregates back into their nodes: one, or ``"all"``."""
        ids = list(self.aggregates) if id == "all" else [id] if id in self.aggregates else []
        for a in ids:
            del self.aggregates[a]
        if ids:
            self.patch({"op": "ungroup", "ids": ids})
        return len(ids)


# --- RDF -> graph elements ---------------------------------------------------

def local_name(iri: Any) -> str:
    s = str(iri)
    cut = max(s.rfind("#"), s.rfind("/"))
    return s[cut + 1:] if 0 <= cut < len(s) - 1 else s


def _strip_brackets(s: str) -> str:
    """``<iri>`` → ``iri`` (each bracket only where it is)."""
    s = s[1:] if s.startswith("<") else s
    return s[:-1] if s.endswith(">") else s


_NOT_IN_IRI = re.compile(r"[\s<>\"{}|\\^`]")


def iri_token(iri: Any) -> str:
    """``<iri>`` for an IRI given bare or bracketed; anything that cannot be
    one is refused."""
    bare = _strip_brackets(str(iri).strip())
    if not bare or _NOT_IN_IRI.search(bare):
        raise ToolError(f"not a usable IRI: {iri}")
    return f"<{bare}>"


@functools.lru_cache(maxsize=1 << 16)
def _parse(token: str) -> Tuple[str, str, Optional[str]]:
    """A term token → (kind, value, lang). Results repeat their terms, so each
    distinct token is parsed once."""
    if token.startswith("<") and not token.startswith("<<") and "\\" not in token:
        return "iri", token[1:-1], None
    t = rete.Term.parse(token)
    return t.kind, t.value, t.lang


def _literal_text(value: str, lang: Optional[str]) -> str:
    return value + (f"@{lang}" if lang else "")


def _display(token: str) -> str:
    kind, value, lang = _parse(token)
    return _literal_text(value, lang) if kind == "literal" else value


def _new_node(token: str) -> Dict[str, Any]:
    kind, value, _ = _parse(token)
    return {"id": token, "iri": value if kind == "iri" else None, "kind": kind,
            "label": None if kind == "iri" else value, "types": [], "props": []}


def graph_of(triples: List[List[Optional[str]]]) -> Dict[str, List[Dict[str, Any]]]:
    """Graph elements from triples of term tokens. Literal objects become
    properties of their subject (the page can draw them as nodes); rdf:type
    becomes a node's types (what "group by type" groups on), not an edge."""
    nodes: Dict[str, Dict[str, Any]] = {}
    edges: List[Dict[str, Any]] = []

    def node(token: str) -> Dict[str, Any]:
        n = nodes.get(token)
        if n is None:
            n = nodes[token] = _new_node(token)
        return n

    for s, p, o in triples:
        ok, ov, olang = _parse(o)
        p_iri = _parse(p)[1] if p else None
        subject = node(s)
        if ok == "literal":
            subject["props"].append([p_iri or "value", _literal_text(ov, olang)])
            if not subject["label"] and p_iri in LABELS:
                subject["label"] = ov
        elif p_iri == RDF_TYPE and ok == "iri":
            if ov not in subject["types"]:
                subject["types"].append(ov)
        else:
            node(o)
            edges.append({"id": f"{s} {p or 'related'} {o}", "source": s, "target": o, "p": p_iri,
                          "label": local_name(p_iri) if p_iri else "related"})
    return {"nodes": list(nodes.values()), "edges": edges}


_COLUMN = "urn:rete:column:"
_NAMING_COLUMN = re.compile(r"\?(label|title|name|prefLabel)", re.IGNORECASE)


def graph_of_rows(variables: List[str], rows: List[Dict[str, str]]) -> Dict[str, List[Dict[str, Any]]]:
    """A SELECT's rows as a graph. Columns shaped like subject / predicate /
    object are read as triples; otherwise the first column is the thing each
    row is about, an IRI in another column an edge named after the column,
    and a literal a property."""
    names = [v.lower() for v in variables]
    if all(v in names for v in ("s", "p", "o")) or all(v in names for v in ("subject", "predicate", "object")):
        s, p, o = ("s", "p", "o") if "s" in names else ("subject", "predicate", "object")
        cs, cp, co = (variables[names.index(n)] for n in (s, p, o))
        return graph_of([[r[cs], r.get(cp), r[co]] for r in rows if r.get(cs) and r.get(co)])
    if not variables:
        return {"nodes": [], "edges": []}
    first, rest = variables[0], variables[1:]
    triples, subjects = [], {}
    for row in rows:
        if not row.get(first):
            continue
        subjects[row[first]] = None
        for v in rest:
            if row.get(v):
                triples.append([row[first], f"<{_COLUMN}{v}>", row[v]])
    graph = graph_of(triples)
    # Rows whose only value is the first column still get their node.
    have = {n["id"] for n in graph["nodes"]}
    graph["nodes"].extend(_new_node(s) for s in subjects if s not in have)
    for n in graph["nodes"]:
        n["props"] = [[p.replace(_COLUMN, "?", 1) if p.startswith(_COLUMN) else p, v] for p, v in n["props"]]
        # A ?title / ?label / ?name column names the row's entity.
        if n["label"] is None:
            named = next((v for p, v in n["props"] if _NAMING_COLUMN.fullmatch(p)), None)
            n["label"] = re.sub(r"@[a-z-]+$", "", named, flags=re.IGNORECASE) if named is not None else None
    for e in graph["edges"]:
        if e["p"] and e["p"].startswith(_COLUMN):
            e["label"] = f"?{e['p'][len(_COLUMN):]}"
            e["p"] = None
    return graph


def _triples_of(env: Dict[str, Any]) -> Optional[List[List[str]]]:
    """A CONSTRUCT or DESCRIBE answer's triples, or None for a SELECT / ASK."""
    if env.get("kind") in ("construct", "describe") or "triples" in env:
        return env.get("triples") or []
    return None


def _term_of(token: str) -> Dict[str, Any]:
    """A cell as the Retelens cells take it: kind, value, datatype, lang (a quoted triple is shown as text)."""
    if token.startswith("<") and not token.startswith("<<") and "\\" not in token:
        return {"kind": "iri", "value": token[1:-1]}
    t = rete.Term.parse(token)
    o: Dict[str, Any] = {"kind": "literal" if t.kind == "triple" else t.kind, "value": t.value}
    if t.datatype:
        o["datatype"] = t.datatype
    if t.lang:
        o["lang"] = t.lang
    return o


def table_of(env: Dict[str, Any]) -> Dict[str, Any]:
    """Table rows: ``rows`` as plain display strings (filter, sort, CSV), ``terms`` the same
    cells typed (the table's rich cells and column statistics, the cards, the map)."""
    triples = _triples_of(env)
    if triples is not None:
        cols = ("subject", "predicate", "object")
        kept = triples[:MAX_ROWS]
        return {"vars": list(cols),
                "rows": [{k: _display(t[i]) for i, k in enumerate(cols)} for t in kept],
                "terms": [{k: _term_of(t[i]) for i, k in enumerate(cols)} for t in kept]}
    variables = list(env.get("vars") or [])
    kept = (env.get("rows") or [])[:MAX_ROWS]
    return {"vars": variables,
            "rows": [{v: _display(r[v]) if r.get(v) is not None else "" for v in variables} for r in kept],
            "terms": [{v: _term_of(r[v]) for v in variables if r.get(v) is not None} for r in kept]}


def elements_of(env: Dict[str, Any]) -> Tuple[Dict[str, List[Dict[str, Any]]], Dict[str, Any]]:
    """Elements and table rows of a finished query's answer."""
    triples = _triples_of(env)
    if triples is not None:
        return graph_of(triples), table_of(env)
    if env.get("kind") == "select":
        return graph_of_rows(list(env.get("vars") or []), env.get("rows") or []), table_of(env)
    raise ToolError("an ASK answer is one boolean: there is no graph to show; use SELECT or CONSTRUCT")


def load_into(view: View, env: Dict[str, Any], mode: str, query: Optional[str] = None) -> Dict[str, Any]:
    """Put a finished query's results into a view: added, or replacing what it holds.
    ``query``, the SPARQL text, rides with the table: its comments can name the columns
    (Retelens query hints, clients/chrome/lib/hints.js)."""
    graph, table = elements_of(env)
    if table and query:
        table["query"] = str(query)[:20000]
    if mode == "replace":
        view.clear()
        added = view.add(graph)
        view.table = table
        view.patch({"op": "replace", "nodes": added["nodes"], "edges": added["edges"], "table": table})
        return {**added, "rows": len(table["rows"]),
                "truncated": len(graph["nodes"]) > MAX_NODES or len(graph["edges"]) > MAX_EDGES}
    added = view.add(graph)
    if view.table and view.table["vars"] == table["vars"]:
        view.table = {"vars": table["vars"],
                      "rows": (view.table["rows"] + table["rows"])[:MAX_ROWS],
                      "terms": ((view.table.get("terms") or []) + (table.get("terms") or []))[:MAX_ROWS],
                      **({"query": view.table["query"]} if view.table.get("query") else {})}
    else:
        view.table = table
    view.patch({"op": "add", "nodes": added["nodes"], "edges": added["edges"], "table": view.table})
    return {**added, "rows": len(table["rows"]),
            "truncated": len(view.nodes) >= MAX_NODES or len(view.edges) >= MAX_EDGES}


def graph_of_schema(schema: Dict[str, Any]) -> Dict[str, Any]:
    """A schema as graph elements: each class a node of kind "class" with its
    instance count, the relations to literals its properties, the relations to
    other classes edges. The most frequent classes only, and what links them."""
    classes = schema.get("classes") or []
    kept = sorted(classes, key=lambda c: -c[1])[:SCHEMA_CLASSES]
    nodes: Dict[str, Dict[str, Any]] = {}

    def node_id(c: str) -> str:
        return "_:untyped" if c == "(untyped)" else f"<{c}>"

    def node(c: str, count: Optional[int] = None) -> Dict[str, Any]:
        i = node_id(c)
        if i not in nodes:
            iri = None if c == "(untyped)" else c
            nodes[i] = {"id": i, "iri": iri, "kind": "class", "label": local_name(iri) if iri else "(untyped)",
                        "types": [], "props": [], "count": count}
        return nodes[i]

    for c, n in kept:
        node(c, n)
    edges, rows = [], []
    for s, p, o, n in sorted(schema.get("relations") or [], key=lambda r: -r[3]):
        if p == RDF_TYPE or node_id(s) not in nodes:
            continue
        if o == "(literal)":
            node(s)["props"].append([p, f"{n:,}"])
        elif node_id(o) in nodes:
            a, b = node(s)["id"], node(o)["id"]
            edges.append({"id": f"{a} <{p}> {b}", "source": a, "target": b, "p": p,
                          "label": f"{local_name(p)} ({n:,})"})
        else:
            continue
        rows.append({"class": local_name(s), "property": local_name(p),
                     "object": "literal" if o == "(literal)" else local_name(o), "count": str(n)})
    return {"nodes": list(nodes.values()), "edges": edges,
            "table": {"vars": ["class", "property", "object", "count"], "rows": rows}, "classes": len(classes)}


def schema_into(view: View, schema: Dict[str, Any]) -> Dict[str, int]:
    """A schema's elements into a view, replacing what it holds."""
    g = graph_of_schema(schema)
    view.clear()
    added = view.add({"nodes": g["nodes"], "edges": g["edges"]})
    view.table = g["table"]
    view.patch({"op": "replace", "nodes": added["nodes"], "edges": added["edges"], "table": g["table"]})
    return {"classes": g["classes"], "shown": len(g["nodes"]), "relations": len(g["edges"])}


def neighbour_queries(node: str) -> List[Tuple[str, Callable[[Dict[str, str]], List[Optional[str]]]]]:
    """The two queries that bring a node's statements, both directions, each
    with how its rows become triples. SELECTs, because the engine does not
    apply LIMIT to a CONSTRUCT: a hub's expansion would read every link it has."""
    return [
        (f"SELECT ?p ?o WHERE {{ {node} ?p ?o }} LIMIT {NEIGHBOURS}", lambda r: [node, r.get("p"), r.get("o")]),
        (f"SELECT ?s ?p WHERE {{ ?s ?p {node} }} LIMIT {NEIGHBOURS}", lambda r: [r.get("s"), r.get("p"), node]),
    ]


def unlabelled(view: View, ids: List[str]) -> List[str]:
    """The IRI nodes of a view, among ``ids``, that still have no label."""
    return [i for i in ids if (n := view.nodes.get(i)) and n["kind"] == "iri" and not n["label"]]


def apply_labels(view: View, found: Dict[str, List[Dict[str, Any]]]) -> int:
    """Labels and types read for some nodes, merged into the view and patched to it."""
    labels = {}
    for n in found["nodes"]:
        had = view.nodes.get(n["id"])
        if had is None:
            continue
        if not had["label"] and n["label"]:
            had["label"] = n["label"]
        for t in n["types"]:
            if t not in had["types"]:
                had["types"].append(t)
        labels[n["id"]] = {"label": had["label"], "types": had["types"]}
    if labels:
        view.patch({"op": "labels", "labels": labels})
    return len(labels)


# --------------------------------------------------------------------------- #
# The tools (views.mjs)
# --------------------------------------------------------------------------- #

class ViewOptions(BaseModel):
    """A graph view's look, shared by graph_view and graph_view_update."""

    diagram: Optional[Literal["graph", "boxes", "classes"]] = Field(
        None, description="how to draw it: graph (default), each entity a dot; boxes, each entity a box with its "
                          "name and type, its properties listed below (a UML object diagram); classes, one box per "
                          "rdf:type listing the properties its members carry, linked by the relations between the "
                          "classes (a UML class diagram). boxes and classes come laid out in layers with square lines")
    labels: Optional[bool] = Field(None, description="show node labels (default true)")
    properties: Optional[bool] = Field(None, description="list properties inside boxes (default true)")
    node_shape: Optional[Literal["circle", "square"]] = Field(None, description="the dots of the graph diagram "
                                                                                "(default circle)")
    edge_style: Optional[Literal["curved", "straight", "square"]] = Field(
        None, description="lines between nodes: square draws orthogonal connectors, as in UML diagrams")
    edge_labels: Optional[bool] = Field(None, description="show predicate names on edges (default false)")
    group_by: Optional[str] = Field(None, description='colour and layer nodes by "type" (rdf:type, default), '
                                                      '"namespace", "none", or a predicate IRI')
    group_boxes: Optional[bool] = Field(None, description="draw each group as a box around its nodes "
                                                          "(default false)")
    layout: Optional[Literal["cose", "layered", "concentric", "breadthfirst", "circle", "grid", "hive"]] = Field(
        None, description="cose: force-directed (default for graph); layered: top-down layers "
                          "(default for boxes and classes); hive: nodes on a few radial axes, ordered along "
                          "each by hive_order, edges curved between the axes")
    hive_axes: Optional[HiveAxes] = Field(
        None, description="hive layout: what puts a node on an axis — type (its class), role (sources / both / "
                          "sinks), kind (resource / literal / …), degree (thirds by links); auto (default) takes "
                          "the first that tells two groups apart")
    hive_order: Optional[HiveOrder] = Field(
        None, description="hive layout: what orders nodes along an axis, from the centre out — degree (links, "
                          "default), in, out, name")
    view: Optional[Literal["graph", "table", "cards", "map"]] = Field(
        None, description="which tab is shown: the graph, the rows as a table (pictures, IIIF manifests, "
                          "geometries and links drawn in the cells, a chart of each column's values under its "
                          "name), the rows as cards (pictures, IIIF, maps, video, audio and 3D models drawn where "
                          "the values are addresses or geometries), or a map (WKT geometries, or latitude and "
                          "longitude columns, on an OpenStreetMap basemap; a column of .pmtiles URLs is drawn as "
                          "layers under them). A query's hints choose it when this is not given: a comment line "
                          "'# view: map'")
    card_fields: Optional[Dict[str, FieldType]] = Field(
        None, description="cards: how to draw each column, by column name (a SELECT variable): "
                          + " / ".join(FIELD_TYPE_NAMES) + ". Columns not named are detected from their values "
                          "(auto)")
    theme: Optional[Literal["auto", "light", "dark"]] = Field(
        None, description="the view's colours; auto (default) follows the client's theme")
    literals_as_nodes: Optional[bool] = Field(None, description="draw literal values as nodes instead of "
                                                                "node properties")


class Aggregate(BaseModel):
    iris: Optional[List[str]] = Field(None, description="the entities to draw as one node")
    type: Optional[str] = Field(None, description="or: every node of this rdf:type (an IRI)")
    label: Optional[str] = Field(None, description="the aggregate's name (default: the type, or a count)")


class EditAggregate(BaseModel):
    ids: List[str]
    label: Optional[str] = None


def _options(options: Optional[ViewOptions]) -> Dict[str, Any]:
    return options.model_dump(exclude_none=True) if options else {}


def _text(doc: Any) -> ToolResult:
    """An answer the page reads: JSON in one text block. Built while the view
    is locked, so a concurrent change cannot tear it."""
    return ToolResult(content=[TextContent(type="text", text=json.dumps(doc, ensure_ascii=False))])


def _errors(fn):
    """Service errors become tool errors with a clean message."""
    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        try:
            return fn(*args, **kwargs)
        except ToolError:
            raise
        except Exception as e:
            raise ToolError(state.describe_error(e)) from None
    return wrapper


class Views:
    """The open views of one MCP server, and the files arriving in them."""

    def __init__(self, uploads: bool):
        self.uploads_on = uploads
        self._views: "OrderedDict[str, View]" = OrderedDict()
        self._uploads: Dict[str, Dict[str, Any]] = {}  # view id -> {name, size, bytes, received, at}
        self._lock = threading.Lock()

    # -- views -----------------------------------------------------------

    def open(self, dataset: Optional[str], title: Optional[str], options: Dict[str, Any]) -> View:
        with self._lock:
            while True:
                vid = f"v{secrets.randbelow(10 ** 12):012d}"  # the page reads a view id as v + digits
                if vid not in self._views:
                    break
            view = self._views[vid] = View(vid, dataset, title, options)
            while len(self._views) > MAX_VIEWS:
                old, _ = self._views.popitem(last=False)
                self._uploads.pop(old, None)
        return view

    def get(self, view_id: Any) -> View:
        vid = str(view_id or "").strip()
        with self._lock:
            view = self._views.get(vid)
            if view is not None:
                self._views.move_to_end(vid)
        if view is None:
            raise ToolError(f"no graph view {view_id}: views last as long as the server runs (the newest "
                            f"{MAX_VIEWS} are kept); open a new one with graph_view")
        return view

    @staticmethod
    def dataset_of(view: View) -> str:
        """A view's dataset; none yet while it waits for the user to open a file."""
        if view.dataset:
            return view.dataset
        raise ToolError(f"graph view {view.id} is still waiting for the user to open "
                        f"{(view.awaiting or {}).get('file') or 'a file'} in it")

    # -- what goes into a view -------------------------------------------

    def query(self, view: View, text: str, mode: str) -> Dict[str, Any]:
        """Run ``text`` into ``view``. The query runs unlocked; its rows go in
        under the view's lock."""
        env = state.service.query_env(self.dataset_of(view), text)
        with view.lock:
            return load_into(view, env, mode, text)

    def expand(self, view: View, iri: str) -> Dict[str, List[Dict[str, Any]]]:
        """A node's statements, both directions, into the view."""
        node = iri_token(iri)
        dataset = self.dataset_of(view)
        triples = []
        for q, triple in neighbour_queries(node):
            env = state.service.query_env(dataset, q)
            if env.get("kind") == "select":
                triples += [triple(r) for r in env.get("rows") or [] if r.get("p") and (r.get("o") or r.get("s"))]
        graph = graph_of(triples)
        with view.lock:
            added = view.add(graph)
            view.patch({"op": "add", "nodes": added["nodes"], "edges": added["edges"], "focus": node})
        return added

    def label(self, view: View, ids: List[str]) -> int:
        """Labels and types of nodes that lack them, read in one query on
        those subjects."""
        with view.lock:
            want = unlabelled(view, ids)[:LABEL_BATCH]
        if not want:
            return 0
        preds = ", ".join(f"<{p}>" for p in [*LABELS, RDF_TYPE])
        q = (f"SELECT ?s ?p ?o WHERE {{ VALUES ?s {{ {' '.join(want)} }} ?s ?p ?o "
             f"FILTER(?p IN ({preds})) }}")
        env = state.service.query_env(self.dataset_of(view), q)
        found = graph_of([[r["s"], r["p"], r["o"]] for r in env.get("rows") or []
                          if r.get("s") and r.get("p") and r.get("o")])
        with view.lock:
            return apply_labels(view, found)

    def schema(self, view: View) -> Dict[str, Any]:
        """A dataset's schema into a view, as boxes of classes and their properties."""
        schema = state.service.schema_lists(self.dataset_of(view))
        with view.lock:
            return {**schema_into(view, schema), "from": schema["from"]}

    # -- files from the user ---------------------------------------------

    def receive(self, view: View, name: str, size: int, offset: int, data: str) -> Tuple[Optional[bytes], int]:
        """One part of a file the user chose or dropped in a view: the whole
        file once its last part is in (else None), and the bytes received."""
        if not self.uploads_on:
            raise ToolError("this server does not take files from a graph view (authoring is off)")
        cap = state.settings.upload_max_mb << 20
        if _format_of(name) not in (None, "jelly"):
            cap = min(cap, TEXT_MAX_BYTES)
        try:
            part = base64.b64decode(data, validate=True)
        except (binascii.Error, ValueError):
            raise ToolError(f"a part of {name} is not base64; open it again") from None
        now = time.time()
        with self._lock:
            for vid in [v for v, u in self._uploads.items() if now - u["at"] > UPLOAD_IDLE_S]:
                del self._uploads[vid]
            up = self._uploads.get(view.id)
            if offset == 0:
                self._uploads.pop(view.id, None)  # a restart replaces the view's unfinished upload
                if size > cap:
                    raise ToolError(f"{name} is {_mb(size)}, more than the {_mb(cap)} a file opened in a "
                                    "graph view may be on this server")
                pending = sum(u["size"] for u in self._uploads.values())
                if pending + size > 4 * (state.settings.upload_max_mb << 20):
                    raise ToolError(f"{name} would bring the files being opened in views to "
                                    f"{_mb(pending + size)}; let the others finish first")
                up = self._uploads[view.id] = {"name": name, "size": size, "bytes": bytearray(size),
                                               "received": 0, "at": now}
            if up is None or up["name"] != name or up["size"] != size or offset != up["received"]:
                raise ToolError(f"the parts of {name} came out of order (at {offset}, expected "
                                f"{up['received'] if up else 0}); open it again")
            if offset + len(part) > size:
                del self._uploads[view.id]
                raise ToolError(f"{name} came out longer than its {size} bytes; open it again")
            up["bytes"][offset:offset + len(part)] = part
            up["received"] += len(part)
            up["at"] = now
            if up["received"] < size:
                return None, up["received"]
            del self._uploads[view.id]
            return bytes(up["bytes"]), size

    def opened(self, view: View, name: str, data: bytes) -> Dict[str, Any]:
        """A file the user opened in a view, whole: a dataset on this server
        (``generated/…``, like a build_rete file), shown in the view."""
        from . import author

        fmt = _format_of(name)
        try:
            if fmt:
                source: Any = data if fmt == "jelly" else data.decode("utf-8")
                data = rete.Builder().add(source, format=fmt).run()
            graph = rete.open(data)
        except UnicodeDecodeError:
            raise ToolError(f"{name} is not UTF-8 text, as {fmt} must be") from None
        except Exception as e:
            what = f"valid {fmt}" if fmt else "a .rete file this server can read, nor RDF text by its name"
            raise ToolError(f"{name} is not {what} ({str(e).splitlines()[0] if str(e) else type(e).__name__})") \
                from None
        key = f"generated/{author.store_generated(data, name)}"
        with view.lock:
            awaited = view.awaiting
            then = view.then or {}
            view.dataset, view.awaiting, view.then = key, None, None
            if not awaited:
                view.title = name
            view.clear()
            view.status = ""
            view.patch({"op": "dataset", "dataset": key, "title": view.title})
        try:
            if then.get("schema"):
                self.schema(view)
            elif then.get("query"):
                self.query(view, then["query"], "replace")
            elif then.get("iri"):
                self.expand(view, then["iri"])
            else:
                self.query(view, SAMPLE, "replace")
        except Exception as e:
            # The file is open all the same; the view says what went wrong.
            with view.lock:
                view.status = (str(e) or type(e).__name__).splitlines()[0]
                view.patch({"op": "status", "status": view.status})
        return {"dataset": key, "statements": graph.quads, "file": name, "awaited": bool(awaited)}


def summary(view: View) -> Dict[str, Any]:
    doc = {"view_id": view.id, "dataset": view.dataset, "nodes": len(view.nodes), "edges": len(view.edges),
           "table_rows": len(view.table["rows"]) if view.table else 0}
    if view.aggregates:
        doc["aggregates"] = [{"id": a["id"], "label": a["label"], "members": len(a["members"])}
                             for a in view.aggregates.values()]
    if view.awaiting:
        doc["awaiting"] = view.awaiting["file"]
    return doc


def user_state(view: View) -> Dict[str, Any]:
    """What the user has done in the view, as the page last reported."""
    if not view.ui:
        return {"open": False, "note": "the view has not reported yet (it may not be shown in this client)"}
    seen = view.seen_at
    return {"open": seen is not None and time.time() - seen < 15,
            "last_seen_s": None if seen is None else round(time.time() - seen), **view.ui}


def register_views(mcp: FastMCP, uploads: bool) -> Views:
    """The graph view: its page, the three tools Claude calls, and the
    app-only tools the page calls."""
    views = Views(uploads)
    page: Dict[str, str] = {}

    # The page carries its script, so it needs the network for nothing but what the card view
    # shows: pictures, audio and video (resourceDomains) and the file of a 3D model
    # (connectDomains), from whatever site a value names. A host only honours the origins
    # declared here, so the list is what can load in the client (media_domains).
    media = media_domains()
    csp = ResourceCSP(resource_domains=media, connect_domains=media) if media else None

    @mcp.resource(VIEW_URI, name="Graph view",
                  description="Interactive graph, table and card view of query results, with labels, layers, "
                              "layouts, zoom and search.",
                  app=AppConfig(prefers_border=True, csp=csp))
    def graph_view_page() -> str:
        if "html" not in page:
            page["html"] = PAGE.read_text(encoding="utf-8")
        return page["html"]

    def await_file(dataset: str, title: Optional[str], look: Dict[str, Any], query: Optional[str],
                   iri: Optional[str], schema: bool) -> ToolResult:
        """A view for a chat attachment this server cannot read: it asks the
        user to choose or drop the file, which graph_view_upload brings here."""
        file = dataset.rstrip("/").rsplit("/", 1)[-1] or "the file"
        if not views.uploads_on:
            raise ToolError(f"{dataset} is a file attached to the chat: it is in Claude's own sandbox, out of "
                            "this server's reach, and this server does not take files from a graph view "
                            "(authoring is off).")
        view = views.open(None, title or file, look)
        view.awaiting = {"file": file}
        view.then = {"query": query, "iri": iri, "schema": schema}
        text = (f"Graph view {view.id} is waiting for {file}: Claude's copy of it is in Claude's own sandbox, out "
                "of this server's reach, so the view asks the user to choose or drop the file. Tell them to. "
                "Once they do, it opens there" + (" with what you asked for" if query or iri or schema else "") +
                ", the view says in the conversation which dataset it became (a generated/… key that works "
                f"with every tool). graph_view_state({{view_id: \"{view.id}\"}}) shows whether it is open yet.")
        return ToolResult(content=[TextContent(type="text", text=text)], structured_content=summary(view))

    # --- tools Claude calls --------------------------------------------------

    @mcp.tool(
        title="Show a graph view",
        annotations={"readOnlyHint": True, "openWorldHint": True},
        app=AppConfig(resource_uri=VIEW_URI),
        output_schema=None,
    )
    @_errors
    def graph_view(
        dataset: str = Field(description="a dataset key from list_datasets, or a public .rete URL"),
        query: Optional[str] = Field(None, description="SPARQL CONSTRUCT or SELECT whose results to show"),
        iri: Optional[str] = Field(None, description="or: an entity whose statements (both directions) to show"),
        schema: Optional[bool] = Field(None, description="or: the dataset's schema: classes as boxes listing "
                                                         "their properties, linked by their relations"),
        title: Optional[str] = Field(None, description="a short title shown above the graph"),
        options: Optional[ViewOptions] = None,
    ) -> ToolResult:
        """Open an interactive graph view in the conversation: the results of a
        SPARQL query (CONSTRUCT, or a SELECT: s/p/o columns are read as
        triples, otherwise the first column is the subject and the others its
        links and properties), or the neighbourhood of one entity (iri). The
        user can toggle node and edge labels, group nodes into coloured layers
        (by rdf:type, namespace or a predicate) and hide layers, change the
        layout, zoom, search, expand a node by double-clicking it, and switch
        to a table. Keep queries to a few hundred nodes (a LIMIT). Answers
        with a view_id: change the open view with graph_view_update and read
        what the user selected with graph_view_state. Also the way to open a
        file attached to the chat (a path like /mnt/user-data/uploads/…, which
        this server cannot read): pass that path as dataset, and the view asks
        the user to choose or drop the file. options.diagram draws it as dots
        (graph), as boxes listing each entity's properties (boxes), or as a
        class diagram (classes); schema=true draws the dataset's own schema,
        its classes as boxes with their properties linked by their relations,
        with no query. Shown only by clients that render MCP Apps (Claude
        does); elsewhere the answer is text."""
        # A schema reads best as boxes of classes, coloured by vocabulary, its relations named.
        opts = _options(options)
        if query:
            opts = with_hinted_view(opts, query)
        look = look_of({"diagram": "boxes", "group_by": "namespace", "edge_labels": True, **opts}
                       if schema else opts)
        if SANDBOX.match(dataset or ""):
            return await_file(dataset, title, look, query, iri, bool(schema))
        _, entry = state.service.source_of(dataset)
        key = entry.get("key") or dataset
        name = title or (f"schema of {key}" if schema else local_name(_strip_brackets(iri)) if iri else "graph")
        view = views.open(key, name, look)
        note = ""
        if schema:
            r = views.schema(view)
            shown = (f"the {r['shown']} most frequent of {r['classes']} classes" if r["classes"] > r["shown"]
                     else f"{r['shown']} classes")
            note = (f" The schema (from its {'card' if r['from'] == 'card' else 'schema pyramid'}): {shown}, "
                    f"{r['relations']} relations between them; each box lists the class's literal properties.")
        elif query:
            r = views.query(view, query, "replace")
            if r["truncated"]:
                note = f" Capped at {MAX_NODES} nodes / {MAX_EDGES} edges: narrow the query."
        elif iri:
            views.expand(view, iri)
        with view.lock:
            s = summary(view)
        text = (f"Graph view {view.id} on {view.dataset}: {s['nodes']} nodes, {s['edges']} edges.{note} It is "
                "shown to the user, who can change labels, layers, layout and zoom, search, expand nodes and "
                f"switch to the table. Change it with graph_view_update({{view_id: \"{view.id}\", …}}); see what "
                f"the user selected with graph_view_state({{view_id: \"{view.id}\"}}).")
        return ToolResult(content=[TextContent(type="text", text=text)], structured_content=s)

    @mcp.tool(
        title="Change a graph view",
        annotations={"readOnlyHint": False, "destructiveHint": False, "openWorldHint": True},
        output_schema=None,
    )
    @_errors
    def graph_view_update(
        view_id: str = Field(description="the view, from graph_view"),
        query: Optional[str] = Field(None, description="SPARQL CONSTRUCT or SELECT whose results to add"),
        mode: Literal["add", "replace"] = Field("add", description="for query: add to the view (default) or "
                                                                   "replace it"),
        expand: Optional[str] = Field(None, description="an entity IRI whose statements to add"),
        highlight: Optional[List[str]] = Field(None, description="entity IRIs to highlight (an empty list "
                                                                 "clears)"),
        focus: Optional[str] = Field(None, description="an entity IRI to centre on"),
        remove: Optional[List[str]] = Field(None, description="entity IRIs to take out of the view"),
        aggregate: Optional[Aggregate] = Field(None, description="draw several nodes as one aggregate node, "
                                                                 "their links merged and counted"),
        ungroup: Optional[str] = Field(None, description='an aggregate id (from graph_view_state) to split back '
                                                         'into its nodes, or "all"'),
        title: Optional[str] = None,
        options: Optional[ViewOptions] = None,
    ) -> ToolResult:
        """Change a graph view the user has open (from graph_view): add the
        results of another query to it (mode add, default) or replace them
        (mode replace), expand an entity's neighbourhood, highlight or focus
        nodes, remove nodes, aggregate several into one node (or every node of
        a type) and ungroup them, or change its look: diagram (graph / boxes /
        classes), labels, edge_labels, properties, node_shape, edge_style
        (curved / straight / square), group_by (type / namespace / none / a
        predicate IRI), group_boxes, layout (incl. hive, with
        hive_axes and hive_order), view (graph / table / cards, with
        card_fields), literals_as_nodes, theme (auto / light / dark). The open view picks
        the change up within a second or two. Answers with the new counts and
        what the user has selected."""
        view = views.get(view_id)
        done = []
        opts = _options(options)
        if opts or title:
            with view.lock:
                view.options.update(look_of(opts, view.options))
                if title:
                    view.title = title
                view.patch({"op": "options", "options": view.options, "title": view.title})
            done.append("look changed")
        if query:
            r = views.query(view, query, mode)
            done.append(f"{'replaced with' if mode == 'replace' else 'added'} {len(r['nodes'])} nodes and "
                        f"{len(r['edges'])} edges")
        if expand:
            r = views.expand(view, expand)
            done.append(f"expanded {expand}: +{len(r['nodes'])} nodes, +{len(r['edges'])} edges")
        with view.lock:
            if remove:
                done.append(f"removed {view.remove([iri_token(i) for i in remove])} nodes")
            if ungroup:
                done.append(f"ungrouped {view.ungroup(ungroup)} aggregate(s)")
            if aggregate:
                if aggregate.type:
                    t = _strip_brackets(aggregate.type)
                    ids = [n["id"] for n in view.nodes.values() if t in n["types"]]
                else:
                    ids = [iri_token(i) for i in aggregate.iris or []]
                label = aggregate.label or (local_name(_strip_brackets(aggregate.type)) if aggregate.type else None)
                try:
                    a = view.aggregate(ids, label)
                except ToolError as e:
                    raise ToolError(f"{e} ({len(ids)} matched)") from None
                done.append(f"aggregated {len(a['members'])} nodes as {a['id']} \"{a['label']}\"")
            if highlight is not None:
                view.patch({"op": "highlight", "ids": [iri_token(i) for i in highlight]})
                done.append(f"highlighted {len(highlight)}" if highlight else "highlight cleared")
            if focus:
                view.patch({"op": "focus", "id": iri_token(focus)})
                done.append("focused")
            return _text({**summary(view), "done": done, "user": user_state(view)})

    @mcp.tool(title="What the user did in a graph view", annotations={"readOnlyHint": True}, output_schema=None)
    @_errors
    def graph_view_state(view_id: str = Field(description="the view, from graph_view")) -> ToolResult:
        """What the user has selected and set in a graph view: the selected
        nodes (IRI and label), the search text, hidden layers, the look
        (labels, grouping, layout, tab), the aggregates and whether the view
        is still open. Use it when the user refers to "this", "these" or "the
        selected" nodes."""
        view = views.get(view_id)
        with view.lock:
            return _text({**summary(view), "title": view.title, "user": user_state(view)})

    # --- app-only tools: what the page itself calls ----------------------------

    app_only = AppConfig(resource_uri=VIEW_URI, visibility=["app"])
    since_field = Field(0, ge=0, description="the version the view has")

    def app_tool(title: str):
        return mcp.tool(title=title, app=app_only, output_schema=None)

    @app_tool("Graph view: sync")
    @_errors
    def graph_view_sync(view_id: str, since: int = since_field,
                        state: Optional[Dict[str, Any]] = None) -> ToolResult:
        """The view's changes since `since`, and the user's state into the server."""
        view = views.get(view_id)
        with view.lock:
            view.seen_at = time.time()
            if state:
                view.ui = state
            return _text(view.since(since))

    @app_tool("Graph view: expand a node")
    @_errors
    def graph_view_expand(view_id: str, iri: str, since: int = since_field) -> ToolResult:
        """Add a node's statements, both directions, to the view."""
        view = views.get(view_id)
        views.expand(view, iri)
        with view.lock:
            return _text(view.since(since))

    @app_tool("Graph view: find entities")
    @_errors
    def graph_view_search(view_id: str, text: str, since: int = since_field) -> ToolResult:
        """Find entities by name in the view's graph and add them to the view."""
        view = views.get(view_id)
        try:
            hits = state.service.entity_search(text, views.dataset_of(view), 20)
        except (ToolError, RequestError, Busy, QueryTimeout):
            raise
        except Exception:  # the engine's own refusal: this file has no label index
            hits = None
        if not hits:
            with view.lock:
                note = None if hits is not None else (f"{view.dataset} has no name index to search; ask Claude "
                                                      "to find it with a query")
                return _text({**view.since(since), "found": 0, **({"note": note} if note else {})})
        nodes = []
        for hit in hits:
            iri = _strip_brackets(str(hit["subject"]))
            nodes.append({"id": f"<{iri}>", "iri": iri, "kind": "iri", "label": hit.get("label"),
                          "types": [], "props": []})
        with view.lock:
            added = view.add({"nodes": nodes, "edges": []})
            view.patch({"op": "add", "nodes": added["nodes"], "edges": [], "highlight": [n["id"] for n in nodes]})
            return _text({**view.since(since), "found": len(nodes)})

    @app_tool("Graph view: labels and types")
    @_errors
    def graph_view_labels(view_id: str, ids: List[str], since: int = since_field) -> ToolResult:
        """Read the labels and rdf:types of nodes that lack them."""
        view = views.get(view_id)
        views.label(view, ids)
        with view.lock:
            return _text(view.since(since))

    @app_tool("Graph view: edit")
    @_errors
    def graph_view_edit(
        view_id: str,
        remove: Optional[List[str]] = Field(None, description="node ids to take out"),
        aggregate: Optional[EditAggregate] = None,
        ungroup: Optional[str] = Field(None, description='an aggregate id, or "all"'),
        since: int = since_field,
    ) -> ToolResult:
        """The user's edits from the view's menu: take nodes out of it,
        aggregate nodes into one, ungroup."""
        view = views.get(view_id)
        with view.lock:
            if remove:
                view.remove(remove)
            if ungroup:
                view.ungroup(ungroup)
            if aggregate:
                view.aggregate(aggregate.ids, aggregate.label)
            return _text(view.since(since))

    @app_tool("Graph view: open a file")
    @_errors
    def graph_view_upload(
        view_id: str,
        name: str = Field(description="the file's name"),
        size: int = Field(ge=1, description="the whole file's bytes"),
        offset: int = Field(ge=0, description="where this part starts"),
        data: str = Field(description="this part, base64"),
        since: int = since_field,
    ) -> ToolResult:
        """A file the user chose or dropped in the view, in base64 parts sent
        in order: the last one opens it as a dataset on this server and shows
        it in the view."""
        view = views.get(view_id)
        whole, received = views.receive(view, name, size, offset, data)
        if whole is None:
            return _text({"received": received, "size": size})
        done = views.opened(view, name, whole)
        with view.lock:
            return _text({**view.since(since), **done})

    return views
