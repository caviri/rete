"""The bridge to the rete engine: opening graphs and running SPARQL.

Everything that touches ``rete_graph`` lives here, so the rest of the add-on
never imports it directly and keeps working (degraded, with a clear message in
the UI) when the wheel is missing.

Graphs are cached by source string for the session. Opening a remote ``.rete``
costs one small ranged read of the header, and every later query fetches only
the byte ranges it touches, so keeping the handle open is what makes browsing a
multi-gigabyte graph from the viewport practical.
"""

from __future__ import annotations

import json
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

#: Populated on first use by :func:`engine`.
_engine = None
_import_error: Optional[str] = None

#: source string -> open Graph handle
_graphs: Dict[str, Any] = {}

INSTALL_HINT = (
    "The rete engine is not installed in this Blender's Python. "
    "Install the add-on as an extension (the wheel ships inside it), or run: "
    "<blender-python> -m pip install rete-graph"
)


def engine():
    """The ``rete_graph`` module, or ``None`` if it cannot be imported."""
    global _engine, _import_error
    if _engine is None and _import_error is None:
        try:
            import rete_graph  # noqa: PLC0415 - deliberately deferred

            _engine = rete_graph
        except Exception as exc:  # pragma: no cover - depends on the install
            _import_error = f"{exc}"
    return _engine


def available() -> bool:
    return engine() is not None


def unavailable_reason() -> str:
    engine()
    return f"{INSTALL_HINT} ({_import_error})" if _import_error else ""


def version() -> str:
    mod = engine()
    return getattr(mod, "__version__", "?") if mod else ""


# --------------------------------------------------------------------- graphs


def open_graph(source: str, *, reopen: bool = False):
    """Open (or return the cached handle for) a local path or ``http(s)`` URL."""
    mod = engine()
    if mod is None:
        raise RuntimeError(unavailable_reason())
    source = source.strip()
    if not source:
        raise ValueError("no graph source set")
    if reopen:
        _graphs.pop(source, None)
    if source not in _graphs:
        _graphs[source] = mod.open(source)
    return _graphs[source]


def close_all() -> None:
    _graphs.clear()


def is_open(source: str) -> bool:
    return source.strip() in _graphs


def card(source: str) -> Dict[str, Any]:
    """The embedded Dataset Card (title, licence, counts, …), or ``{}``."""
    try:
        return open_graph(source).card() or {}
    except Exception:
        return {}


def examples(source: str) -> List[Dict[str, Any]]:
    """Runnable example queries the file ships with."""
    try:
        return open_graph(source).examples()
    except Exception:
        return []


def stats(source: str) -> Dict[str, Any]:
    """Cumulative bytes/requests actually fetched since the graph was opened."""
    try:
        return open_graph(source).stats()
    except Exception:
        return {}


# --------------------------------------------------------------------- querying

#: One query solution: variable name -> :class:`Cell`.
Row = Dict[str, "Cell"]


class Cell:
    """One RDF term, flattened to what the add-on needs.

    Wraps the client's ``Term`` so downstream code can stay ignorant of the
    engine: ``kind`` is ``iri`` / ``literal`` / ``bnode`` / ``triple``, ``value``
    is the lexical form, and ``python`` is the closest Python value.
    """

    __slots__ = ("kind", "value", "datatype", "lang")

    def __init__(self, kind: str, value: str, datatype: str = "", lang: str = ""):
        self.kind = kind
        self.value = value
        self.datatype = datatype or ""
        self.lang = lang or ""

    @classmethod
    def from_term(cls, term) -> "Cell":
        return cls(term.kind, term.value, term.datatype or "", term.lang or "")

    @property
    def is_iri(self) -> bool:
        return self.kind == "iri"

    def as_number(self) -> Optional[float]:
        try:
            return float(self.value)
        except (TypeError, ValueError):
            return None

    @property
    def python(self) -> Any:
        n = self.as_number() if self.kind == "literal" else None
        return self.value if n is None else n

    def __str__(self) -> str:
        return self.value

    def __repr__(self) -> str:
        return f"Cell({self.kind}, {self.value!r})"


class Result:
    """A SELECT result set: ordered ``vars`` plus ``rows``."""

    def __init__(self, vars_: Sequence[str], rows: List[Row], query: str = "", source: str = ""):
        self.vars: List[str] = list(vars_)
        self.rows: List[Row] = rows
        self.query = query
        self.source = source

    def __len__(self) -> int:
        return len(self.rows)

    def column(self, var: str) -> List[Optional[Cell]]:
        return [row.get(var) for row in self.rows]


def select(source: str, sparql: str, *, reason: bool = False) -> Result:
    """Run a SELECT query and return a :class:`Result`.

    CONSTRUCT/DESCRIBE results are folded into the same shape with ``s``/``p``/
    ``o`` columns, and ASK into a single ``result`` column, so one code path
    downstream handles whatever the user typed.
    """
    graph = open_graph(source)
    env = graph.query_raw(sparql, reason=reason)
    kind = env.get("kind")
    mod = engine()
    parse = mod.Term.parse

    if kind == "select":
        rows = [
            {var: Cell.from_term(parse(tok)) for var, tok in row.items()}
            for row in env.get("rows", [])
        ]
        return Result(env.get("vars", []), rows, sparql, source)
    if kind == "construct":
        rows = [
            dict(zip(("s", "p", "o"), (Cell.from_term(parse(t)) for t in triple)))
            for triple in env.get("triples", [])
        ]
        return Result(["s", "p", "o"], rows, sparql, source)
    if kind == "ask":
        value = "true" if env.get("boolean") else "false"
        cell = Cell("literal", value, "http://www.w3.org/2001/XMLSchema#boolean")
        return Result(["result"], [{"result": cell}], sparql, source)
    raise ValueError(f"unexpected result kind: {kind!r}")


def _values_block(iris: Sequence[str], var: str = "?s") -> str:
    return "VALUES %s { %s }" % (var, " ".join("<%s>" % i for i in iris))


def _batched(items: Sequence[str], size: int) -> Iterable[Sequence[str]]:
    for start in range(0, len(items), size):
        yield items[start : start + size]


#: How many IRIs go into one ``VALUES`` block. Large enough that a few thousand
#: entities take a handful of round trips, small enough to keep each query's
#: parse time and result payload modest.
BATCH = 250


def describe_many(
    source: str,
    iris: Sequence[str],
    *,
    batch: int = BATCH,
    reason: bool = False,
) -> Dict[str, List[Tuple[str, Cell]]]:
    """Every outgoing statement about each IRI: ``{iri: [(predicate, object)]}``.

    This is the "inherit the properties" pass. It batches with ``VALUES`` so a
    thousand entities cost a few queries rather than a thousand, and on a remote
    graph it still only reads the ranges those subjects occupy.
    """
    out: Dict[str, List[Tuple[str, Cell]]] = {i: [] for i in iris}
    for chunk in _batched(list(iris), batch):
        q = "SELECT ?s ?p ?o WHERE { %s ?s ?p ?o }" % _values_block(chunk)
        for row in select(source, q, reason=reason).rows:
            subject, predicate, obj = row.get("s"), row.get("p"), row.get("o")
            if subject is None or predicate is None or obj is None:
                continue
            out.setdefault(subject.value, []).append((predicate.value, obj))
    return out


def pairs_by_predicate(
    source: str,
    iris: Sequence[str],
    predicate: str,
    *,
    inverse: bool = False,
    batch: int = BATCH,
) -> List[Tuple[str, Cell]]:
    """``[(subject_iri, object_cell)]`` for one predicate over the given IRIs.

    Used for hierarchy (``?child partOf ?parent``) and for relation-derived
    physics constraints. ``inverse`` walks the predicate backwards.
    """
    found: List[Tuple[str, Cell]] = []
    pattern = "?o <%s> ?s" % predicate if inverse else "?s <%s> ?o" % predicate
    for chunk in _batched(list(iris), batch):
        q = "SELECT ?s ?o WHERE { %s %s }" % (_values_block(chunk), pattern)
        for row in select(source, q).rows:
            subject, obj = row.get("s"), row.get("o")
            if subject is not None and obj is not None:
                found.append((subject.value, obj))
    return found


def predicates_of(source: str, iris: Sequence[str], limit: int = 400) -> List[Tuple[str, int]]:
    """Predicates used by a sample of the given entities, most frequent first.

    Feeds the "which predicate is the hierarchy / the relation" dropdowns
    without making the user know the vocabulary up front.
    """
    sample = list(iris)[:limit]
    if not sample:
        return []
    q = (
        "SELECT ?p (COUNT(*) AS ?n) WHERE { %s ?s ?p ?o } "
        "GROUP BY ?p ORDER BY DESC(?n)" % _values_block(sample)
    )
    try:
        rows = select(source, q).rows
    except Exception:
        return []
    out = []
    for row in rows:
        p, n = row.get("p"), row.get("n")
        if p is not None:
            out.append((p.value, int(n.as_number() or 0) if n else 0))
    return out


def labels_for(source: str, iris: Sequence[str], batch: int = BATCH) -> Dict[str, str]:
    """Best available human label per IRI (``rdfs:label``, then common
    alternatives). Missing entries simply do not appear."""
    label_props = (
        "http://www.w3.org/2000/01/rdf-schema#label",
        "http://schema.org/name",
        "http://purl.org/dc/terms/title",
        "http://www.w3.org/2004/02/skos/core#prefLabel",
        "http://xmlns.com/foaf/0.1/name",
    )
    union = " UNION ".join("{ ?s <%s> ?l }" % p for p in label_props)
    out: Dict[str, str] = {}
    for chunk in _batched(list(iris), batch):
        q = "SELECT ?s ?l WHERE { %s %s }" % (_values_block(chunk), union)
        try:
            rows = select(source, q).rows
        except Exception:
            return out
        for row in rows:
            s, l = row.get("s"), row.get("l")
            if s is None or l is None:
                continue
            # First one wins, except that an English literal beats a bare one.
            if s.value not in out or l.lang.startswith("en"):
                out[s.value] = l.value
    return out


def json_dumps(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))
