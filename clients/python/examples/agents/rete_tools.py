"""A framework-agnostic tool surface over one `.rete` graph.

Five tools mirror the read loop the rete MCP server exposes — card → schema →
examples → find_entities → sparql_query — but run **in process**, against a
local path or an `https://` URL, with no server between the agent and the file.

Each method returns a JSON string, because that is what every agent framework
wants to hand a model. Results are trimmed: a real schema is hundreds of
kilobytes, which would swamp the context window without telling the model
anything it can act on.

Reads are byte ranges, so the cost of a tool call is bounded by the *query*,
not the file: `tools.read_stats()` reports the bytes and requests actually
fetched so far.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List

import rete_graph as rete

MAX_ROWS = 100  # rows handed back to the model
MAX_CLASSES = 40  # schema trimming — the full profile is far too big to prompt with
MAX_RELATIONS = 60
MAX_EXAMPLES = 8


def _json(obj: Any) -> str:
    return json.dumps(obj, ensure_ascii=False, default=str)


def _value(term: Any) -> Any:
    """A `Term` as a plain JSON value: typed literals become int/float/bool."""
    try:
        return term.to_python()
    except Exception:  # unusual datatype — keep the lexical form
        return term.value


class ReteGraphTools:
    """The five tools, bound to one graph.

    >>> tools = ReteGraphTools("https://data.graphplaza.com/boe/boe.rete")
    >>> tools.dataset_card()[:40]
    '{"title": "BOE — Spanish consolidated l'
    """

    def __init__(self, source: str, *, max_rows: int = MAX_ROWS, **open_kwargs: Any):
        self.source = source
        self.max_rows = max_rows
        self.graph = rete.open(source, **open_kwargs)

    # -- discovery ---------------------------------------------------------

    def dataset_card(self) -> str:
        """What this graph is: title, description, licence, size, vocabularies."""
        card = self.graph.card() or {}
        keep = (
            "title",
            "description",
            "license",
            "source",
            "created",
            "triple_count",
            "quad_count",
            "term_count",
            "vocabularies",
            "languages",
        )
        out: Dict[str, Any] = {k: card[k] for k in keep if card.get(k)}
        out["info"] = self.graph.info()
        return _json(out)

    def dataset_schema(self) -> str:
        """The classes and subject→predicate→object relations actually present.

        Copy these IRIs verbatim into queries; do not invent namespaces.
        """
        schema = self.graph.schema()
        classes = sorted(schema.get("classes", []), key=lambda c: -c[1])[:MAX_CLASSES]
        relations = sorted(schema.get("relations", []), key=lambda r: -r[3])[:MAX_RELATIONS]
        return _json(
            {
                "classes": [{"class": c[0], "instances": c[1]} for c in classes],
                "relations": [
                    {"subject_class": r[0], "predicate": r[1], "object_class": r[2], "count": r[3]}
                    for r in relations
                ],
                "note": (
                    f"top {len(classes)}/{len(schema.get('classes', []))} classes and "
                    f"{len(relations)}/{len(schema.get('relations', []))} relations, by count"
                ),
            }
        )

    def example_queries(self) -> str:
        """Runnable SPARQL the dataset ships with — the graph's own idioms."""
        examples = self.graph.examples() or []
        return _json(
            [
                {"question": e.get("question") or e.get("title"), "sparql": e.get("sparql")}
                for e in examples[:MAX_EXAMPLES]
            ]
        )

    # -- lookup ------------------------------------------------------------

    def find_entities(self, text: str) -> str:
        """Resolve a name to entity IRIs. Use before querying for a named thing."""
        hits: List[List[str]] = list(self.graph.prefix_search(text) or [])
        if not hits:
            # only present when the file was built with --text-index; [] otherwise
            hits = list(self.graph.text_search(text) or [])
        return _json([{"label": h[0], "iri": h[1]} for h in hits[:20]])

    # -- query -------------------------------------------------------------

    def sparql_query(self, query: str) -> str:
        """Run SPARQL 1.1 (SELECT / ASK / CONSTRUCT / DESCRIBE) over the graph.

        A syntax or vocabulary mistake comes back as `{"error": ...}` with the
        parser's message — read it and retry rather than giving up.
        """
        try:
            result = self.graph.query(query)
        except (ValueError, RuntimeError) as exc:
            return _json({"error": str(exc)})

        if isinstance(result, bool):
            return _json({"kind": "ask", "boolean": result})
        if result and isinstance(result[0], tuple):
            triples = [[_value(t) for t in row] for row in result[: self.max_rows]]
            return _json(
                {"kind": "graph", "triples": triples, "truncated": len(result) > self.max_rows}
            )
        rows = [{k: _value(v) for k, v in row.items()} for row in result[: self.max_rows]]
        return _json({"kind": "table", "rows": rows, "truncated": len(result) > self.max_rows})

    # -- observability -----------------------------------------------------

    def read_stats(self) -> Dict[str, Any]:
        """Bytes and requests physically fetched so far — laziness, measured."""
        return self.graph.stats()


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
