"""Query local and remote ``.rete`` graph files with SPARQL.

A ``.rete`` file is a single, immutable, range-queryable RDF graph file: put it
on any HTTP host that supports ``Range`` requests and query it directly — no
server, no database. This package binds the same Rust engine that powers the
rete CLI and the browser playground.

    import rete_graph as rete

    g = rete.open("data/example.rete")                     # local file (lazy)
    g = rete.open("https://example.org/data.rete")         # remote (HTTP range)
    g = rete.open(rete.build(ntriples_text))               # in-memory build

    rows = g.query("SELECT ?s ?label WHERE { ?s rdfs:label ?label } LIMIT 10")
    for row in rows:
        print(row["s"].value, row["label"].to_python())
"""

from __future__ import annotations

import io
import json
import os
import sys
from dataclasses import dataclass
from typing import Any, Dict, Iterator, List, Mapping, Optional, Tuple, Union

from . import _rete

__version__ = _rete.__version__
#: Version of the Rust engine compiled into this wheel. Tracks `__version__`'s
#: major.minor; the patch components move independently.
__engine_version__ = _rete.__engine_version__
__all__ = ["open", "build", "Builder", "Graph", "Term", "DEFAULT_GRAPH"]


class _DefaultGraph:
    """Sentinel for ``graph=``: *the unnamed default graph only*.

    ``graph=None`` means "the whole dataset" (default graph + every named
    graph), so the default graph on its own needs a value of its own —
    :data:`DEFAULT_GRAPH`.
    """

    __slots__ = ()

    def __repr__(self) -> str:  # pragma: no cover - cosmetic
        return "rete_graph.DEFAULT_GRAPH"


#: Pass as ``graph=`` to restrict to the unnamed default graph (see
#: :meth:`Graph.iter_quads`).
DEFAULT_GRAPH = _DefaultGraph()

#: Quads resolved per underlying Rust call by :meth:`Graph.iter_quads` and
#: :meth:`Graph.to_nquads` — the memory ceiling of both. Defined by the
#: binding so the two cannot drift apart.
_BATCH = _rete.DUMP_BATCH

#: Serialized N-Quads lines :meth:`Graph.to_nquads` joins per ``write`` call.
#: Deliberately small and independent of ``batch_size``: joining only exists to
#: amortize write calls, and it costs a temporary as large as the run itself,
#: so a big buffer would inflate peak memory for no throughput.
_NQ_WRITE_LINES = 1024

_XSD = "http://www.w3.org/2001/XMLSchema#"
_INT_TYPES = frozenset(
    _XSD + t
    for t in (
        "integer", "long", "int", "short", "byte",
        "nonNegativeInteger", "nonPositiveInteger", "negativeInteger",
        "positiveInteger", "unsignedLong", "unsignedInt", "unsignedShort",
        "unsignedByte",
    )
)
_FLOAT_TYPES = frozenset(_XSD + t for t in ("decimal", "double", "float"))


@dataclass(frozen=True)
class Term:
    """One RDF term from a query solution.

    ``kind`` is ``"iri"``, ``"literal"``, ``"bnode"`` or ``"triple"`` (an
    RDF-star quoted triple, kept in its N-Triples surface form). ``value`` is
    the IRI, the literal's lexical form, the blank-node label, or the quoted
    triple text.
    """

    kind: str
    value: str
    datatype: Optional[str] = None
    lang: Optional[str] = None

    @staticmethod
    def parse(token: str) -> "Term":
        """Parse an N-Triples-style term token as emitted by the engine."""
        if token.startswith("<<"):
            return Term("triple", token)
        if token.startswith("<") and token.endswith(">"):
            return Term("iri", token[1:-1])
        if token.startswith("_:"):
            return Term("bnode", token)
        if token.startswith('"'):
            return _parse_literal(token)
        # Not a recognized token shape (shouldn't happen); keep it verbatim.
        return Term("literal", token)

    def to_python(self) -> Any:
        """The closest Python value: int/float/bool for the common XSD types,
        otherwise the string ``value`` (dates and times stay strings)."""
        if self.kind == "literal" and self.datatype:
            if self.datatype in _INT_TYPES:
                return int(self.value)
            if self.datatype in _FLOAT_TYPES:
                return float(self.value)
            if self.datatype == _XSD + "boolean":
                return self.value == "true"
        return self.value

    @property
    def n3(self) -> str:
        """The term back in N-Triples surface form."""
        if self.kind == "iri":
            return f"<{self.value}>"
        if self.kind in ("bnode", "triple"):
            return self.value
        body = (
            self.value.replace("\\", "\\\\")
            .replace('"', '\\"')
            .replace("\n", "\\n")
            .replace("\r", "\\r")
        )
        if self.lang:
            return f'"{body}"@{self.lang}'
        if self.datatype:
            return f'"{body}"^^<{self.datatype}>'
        return f'"{body}"'

    def __str__(self) -> str:
        return self.value


def _unescape(s: str) -> str:
    """Resolve N-Triples escape sequences in a literal body."""
    if "\\" not in s:
        return s
    out: List[str] = []
    i, n = 0, len(s)
    simple = {"t": "\t", "b": "\b", "n": "\n", "r": "\r", "f": "\f",
              '"': '"', "'": "'", "\\": "\\"}
    while i < n:
        c = s[i]
        if c != "\\" or i + 1 >= n:
            out.append(c)
            i += 1
            continue
        e = s[i + 1]
        if e in simple:
            out.append(simple[e])
            i += 2
        elif e == "u" and i + 6 <= n:
            out.append(chr(int(s[i + 2 : i + 6], 16)))
            i += 6
        elif e == "U" and i + 10 <= n:
            out.append(chr(int(s[i + 2 : i + 10], 16)))
            i += 10
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _parse_literal(token: str) -> Term:
    # Find the closing quote of the lexical form, skipping escapes.
    i, n = 1, len(token)
    while i < n:
        c = token[i]
        if c == "\\":
            i += 2
            continue
        if c == '"':
            break
        i += 1
    lex = _unescape(token[1:i])
    rest = token[i + 1 :]
    if rest.startswith("@"):
        return Term("literal", lex, lang=rest[1:])
    if rest.startswith("^^<") and rest.endswith(">"):
        return Term("literal", lex, datatype=rest[3:-1])
    return Term("literal", lex)


Row = Dict[str, Term]
Triple = Tuple[Term, Term, Term]
#: One statement from :meth:`Graph.iter_quads`: subject, predicate, object as
#: N-Triples term tokens, plus the graph token (``None`` = default graph).
Quad = Tuple[str, str, str, Optional[str]]


class _EncodingWriter:
    """Adapts a binary stream to ``write(str)``, encoding as UTF-8 — what
    N-Quads requires, and what lets :meth:`Graph.to_nquads` accept
    ``io.BytesIO`` / ``gzip.open(...)`` alongside text streams."""

    __slots__ = ("_raw",)

    def __init__(self, raw: Any):
        self._raw = raw

    def write(self, text: str) -> int:
        return self._raw.write(text.encode("utf-8"))


def _open_text_sink(dest: Any) -> Tuple[Any, Any]:
    """``(writer, close)`` for a path or an already-open text/binary stream.

    Only a stream this function opened is closed again — a caller's
    ``sys.stdout`` or ``StringIO`` is left open.
    """
    if hasattr(dest, "write"):
        try:
            dest.write("")  # cheap probe: a no-op on text, TypeError on binary
        except TypeError:
            return _EncodingWriter(dest), lambda: None
        return dest, lambda: None
    handle = io.open(os.fspath(dest), "w", encoding="utf-8", newline="\n")
    return handle, handle.close


class Graph:
    """A ``.rete`` graph opened for querying (local, remote, or in-memory)."""

    def __init__(self, inner: "_rete.Graph", source: str):
        self._g = inner
        self.source = source

    # -- querying ---------------------------------------------------------

    def query(self, query: str, *, reason: bool = False) -> Union[List[Row], bool, List[Triple]]:
        """Run a SPARQL query.

        Returns a list of ``{variable: Term}`` rows for SELECT, a ``bool`` for
        ASK, and a list of ``(s, p, o)`` Term triples for CONSTRUCT/DESCRIBE.
        ``reason=True`` turns on OWL 2 QL entailment (computed by query
        rewriting over the file's ontology — no materialization).
        """
        env = self.query_raw(query, reason=reason)
        kind = env.get("kind")
        if kind == "select":
            return [
                {var: Term.parse(tok) for var, tok in row.items()}
                for row in env["rows"]
            ]
        if kind == "ask":
            return env["boolean"]
        if kind == "construct":
            return [tuple(Term.parse(t) for t in triple) for triple in env["triples"]]
        raise ValueError(f"unexpected result kind: {kind!r}")

    def query_raw(self, query: str, *, reason: bool = False) -> Dict[str, Any]:
        """The engine's raw JSON result envelope, as a dict."""
        return json.loads(self._g.query(query, reason=reason))

    def query_df(self, query: str, *, reason: bool = False):
        """SELECT results as a pandas DataFrame (needs the ``pandas`` extra).

        Terms are converted with :meth:`Term.to_python`.
        """
        import pandas  # deferred: only needed by this method

        env = self.query_raw(query, reason=reason)
        if env.get("kind") != "select":
            raise ValueError("query_df expects a SELECT query")
        vars_, rows = env["vars"], env["rows"]
        data = {
            v: [Term.parse(r[v]).to_python() if v in r else None for r in rows]
            for v in vars_
        }
        return pandas.DataFrame(data, columns=vars_)

    # -- streaming the whole graph out --------------------------------------

    def iter_quads(
        self,
        graph: Union[str, "_DefaultGraph", None] = None,
        *,
        batch_size: int = _BATCH,
    ) -> Iterator[Quad]:
        """Lazily yield every quad as ``(s, p, o, g)`` **N-Triples tokens**.

        A generator, not a list: the graph is walked in slices of
        ``batch_size`` quads (default ``10_000``), so the quads in flight are
        one slice's worth however large the file is — flat in the quad count,
        where ``list(g.iter_quads())`` is linear in it. (Total process memory
        also carries the engine's faulted index tiles and dictionary chunks,
        which grow with the *file's* size, not with how far the walk has got.)
        Abandoning the iterator (``break``, :func:`itertools.islice`) stops the
        walk immediately; on a remote graph only the byte ranges actually
        consumed are fetched.

        ``s``/``p``/``o`` are canonical N-Triples tokens (``<iri>``,
        ``"lit"@en``, ``"lit"^^<dt>``, ``_:b0``) — the lossless surface form,
        ready to concatenate into N-Triples/N-Quads or hand to another parser.
        Use :meth:`Term.parse` for structure. ``g`` is ``None`` for the default
        graph, else the named graph's ``<iri>`` token.

        ``graph`` selects the scope: ``None`` (default) walks the **whole
        dataset** — default graph first, then each named graph;
        :data:`DEFAULT_GRAPH` walks only the unnamed default graph; an IRI
        (bare or ``<bracketed>``) walks only that named graph. An unknown IRI
        yields nothing.

            for subject, predicate, obj, named_graph in g.iter_quads():
                ...

        ``batch_size`` is the granularity of the walk, so it is also what the
        *first* quad costs: peeking at a huge remote graph is much cheaper with
        a small batch (on a 221 MB / 15.9 M-quad file, five quads cost 1.1 MB
        of range reads at ``batch_size=1`` versus 44 MB at the default), while
        a full walk is fastest at the default. Quads are identical either way.
        """
        for name in self._dump_targets(graph):
            cursor, done = 0, False
            while not done:
                triples, cursor, done = self._g.dump_batch(name, cursor, batch_size)
                for s, p, o in triples:
                    yield (s, p, o, name)

    def to_nquads(
        self,
        dest: Any,
        *,
        graph: Union[str, "_DefaultGraph", None] = None,
        batch_size: int = _BATCH,
    ) -> int:
        """Serialize the graph as N-Quads to ``dest``; return the quad count.

        ``dest`` is a path (written UTF-8) or any open stream, text or binary —
        including ``io.StringIO``, ``sys.stdout``, and ``gzip.open(...)``.
        Streams the graph through :meth:`iter_quads`, so a file far larger than
        RAM serializes in bounded memory. N-Quads is the lossless interchange
        format every RDF toolchain reads, which makes handing a ``.rete`` to
        rdflib, Oxigraph, or Jena a three-liner:

            g.to_nquads("out.nq")
            store = pyoxigraph.Store()
            store.bulk_load(path="out.nq", format=pyoxigraph.RdfFormat.N_QUADS)
        """
        stream, close = _open_text_sink(dest)
        count = 0
        try:
            # Join lines in fixed-size runs: one write call per few thousand
            # quads instead of one per quad, without letting the formatted text
            # in flight outgrow the quads that produced it.
            lines: List[str] = []
            for s, p, o, g in self.iter_quads(graph, batch_size=batch_size):
                lines.append(f"{s} {p} {o} .\n" if g is None else f"{s} {p} {o} {g} .\n")
                count += 1
                if len(lines) >= _NQ_WRITE_LINES:
                    stream.write("".join(lines))
                    lines.clear()
            if lines:
                stream.write("".join(lines))
        finally:
            close()
        return count

    def _dump_targets(self, graph: Union[str, "_DefaultGraph", None]) -> List[Optional[str]]:
        """The graphs one dump covers, as engine tokens (``None`` = default)."""
        if isinstance(graph, _DefaultGraph):
            return [None]
        if graph is None:
            return [None] + list(self._g.graph_names())
        # Accept a bare IRI as well as the `<iri>` token the engine stores.
        return [graph if graph.startswith(("<", "_:")) else f"<{graph}>"]

    # -- validation --------------------------------------------------------

    def shacl(
        self, shapes: str, *, graph: Optional[str] = None, format: str = "json"
    ) -> Union[Dict[str, Any], str]:
        """Validate the graph against SHACL Core shapes written in Turtle.

        Returns the validation report as a dict (``format="json"``, default)
        or as a Turtle string (``format="ttl"``). Lazy-aware: on a remote
        graph only the shapes' targets are fetched — validation reads the
        index in place, never the whole file. A named ``graph`` view
        materializes that graph first.
        """
        out = self._g.shacl(shapes, graph=graph, format=format)
        return json.loads(out) if format == "json" else out

    # -- search & overview -------------------------------------------------

    def prefix_search(self, prefix: str, limit: int = 20) -> List[Tuple[str, str]]:
        """Label prefix search: ``[(label, subject_iri), ...]``."""
        return [
            (label, Term.parse(subject).value)
            for label, subject in self._g.prefix_search(prefix, limit)
        ]

    def text_search(
        self,
        words: Union[str, List[str]],
        contains: Optional[str] = None,
        limit: int = 100,
    ) -> List[str]:
        """Full-text search over the file's TEXT_INDEX; returns subject IRIs.

        The index is opt-in at build time: ``Builder().text_index()`` here, or
        ``rete build --text-index`` in the CLI.
        """
        if isinstance(words, str):
            words = words.split()
        return [Term.parse(s).value for s in self._g.text_search(words, contains, limit)]

    def schema(self) -> Dict[str, Any]:
        """Class and predicate profile: which classes exist, how they relate.

        ``{"classes": [(iri, count), ...],
           "relations": [(s_class, predicate, o_class, count), ...]}``
        """
        env = json.loads(self._g.schema())
        value = lambda tok: Term.parse(tok).value  # engine emits `<iri>` tokens
        return {
            "classes": [(value(c), n) for c, n in env["classes"]],
            "relations": [
                (value(s), value(p), value(o), n) for s, p, o, n in env["relations"]
            ],
        }

    # -- metadata -----------------------------------------------------------

    @property
    def quads(self) -> int:
        return self._g.quads

    @property
    def terms(self) -> int:
        return self._g.terms

    def info(self) -> Dict[str, Any]:
        return json.loads(self._g.info())

    def graph_names(self) -> List[str]:
        # The engine emits `<iri>` tokens; hand back clean IRIs like Term does.
        return [Term.parse(name).value for name in self._g.graph_names()]

    def stats(self) -> Dict[str, Any]:
        """Cumulative physical fetch counters (bytes/requests) since open."""
        return json.loads(self._g.stats())

    def content_hash(self) -> str:
        return self._g.content_hash()

    def card(self) -> Optional[Dict[str, Any]]:
        """The embedded Dataset Card as a dict, or ``None`` if the file has
        none. On lazy opens only the metadata section's byte range is fetched."""
        raw = self._g.card()
        return json.loads(raw) if raw else None

    def examples(self) -> List[Dict[str, Any]]:
        """The example SPARQL queries embedded in the file's Dataset Card.

        Each entry has at least ``"sparql"``; rich entries (written by
        ``Builder.example()`` or generated by the ``rete build`` CLI) also
        carry ``"title"``, ``"question"``, ``"dimension"``, and ``"tier"``.
        Run one with ``g.query(g.examples()[0]["sparql"])``. Empty when the
        file has no card. On lazy/remote opens this costs one small ranged
        read of the metadata section.
        """
        card = self.card() or {}
        rich = [dict(q) for q in card.get("queries", [])]
        legacy = [{"sparql": s} for s in card.get("example_queries", [])]
        return rich + legacy

    def __repr__(self) -> str:
        return f"<rete_graph.Graph source={self.source!r} quads={self.quads}>"


class _XhrRangeReader:
    """HTTP `Range` reader for Pyodide (JupyterLite, marimo WASM): synchronous
    ``XMLHttpRequest`` with a binary response, which browsers allow only in
    web workers — where those tools run their kernels. Same contract as the
    native reader: the server must answer 206, short bodies are errors."""

    def __init__(self, url: str, headers: Optional[Mapping[str, str]] = None):
        from js import XMLHttpRequest  # only importable under Pyodide

        self._xhr_cls = XMLHttpRequest
        self._url = url
        self._headers = dict(headers or {})
        probe = self._request("HEAD", None)
        length = probe.getResponseHeader("Content-Length")
        if length:
            self._len = int(length)
        else:
            # Some hosts omit Content-Length on HEAD; a 1-byte ranged GET's
            # `Content-Range: bytes 0-0/TOTAL` carries the total instead.
            probe = self._request("GET", (0, 0))
            content_range = probe.getResponseHeader("Content-Range") or ""
            if "/" not in content_range:
                raise IOError(f"could not determine the file length of {url}")
            self._len = int(content_range.rsplit("/", 1)[-1])

    def _request(self, method: str, byte_range: Optional[Tuple[int, int]]):
        xhr = self._xhr_cls.new()
        xhr.open(method, self._url, False)  # False = synchronous
        if method == "GET":
            xhr.responseType = "arraybuffer"
        for key, value in self._headers.items():
            xhr.setRequestHeader(key, value)
        if byte_range is not None:
            xhr.setRequestHeader("Range", f"bytes={byte_range[0]}-{byte_range[1]}")
        xhr.send()
        return xhr

    def len(self) -> int:
        return self._len

    def read_at(self, offset: int, length: int) -> bytes:
        if length == 0:
            return b""
        xhr = self._request("GET", (offset, offset + length - 1))
        if xhr.status != 206:
            raise IOError(
                f"server ignored Range (status {xhr.status}, expected 206) for "
                f"{self._url}; the host must support HTTP range requests"
            )
        import js

        data = bytes(js.Uint8Array.new(xhr.response).to_py())
        if len(data) != length:
            raise IOError(
                f"short range response: got {len(data)} of {length} bytes "
                f"at offset {offset} from {self._url}"
            )
        return data


def open(
    source: Union[str, bytes, bytearray, memoryview, "os.PathLike[str]", None] = None,
    *,
    headers: Optional[Mapping[str, str]] = None,
    reader: Any = None,
) -> Graph:
    """Open a ``.rete`` graph.

    ``source`` may be a local path, an ``http(s)://`` URL (queried lazily via
    HTTP range requests — only the byte ranges a query touches are fetched),
    or a ``bytes`` file image. Alternatively pass ``reader=``: any object with
    ``read_at(offset, length) -> bytes`` and ``len()`` (or ``__len__``), e.g.
    an fsspec file wrapper for authenticated S3/GCS.

    ``headers`` (URL sources only) ride on every HTTP request.
    """
    if reader is not None:
        if source is not None:
            raise TypeError("pass either source or reader=, not both")
        return Graph(_rete.open_reader(reader), f"<reader {type(reader).__name__}>")
    if source is None:
        raise TypeError("open() needs a path, URL, bytes, or reader=")
    if isinstance(source, (bytes, bytearray, memoryview)):
        return Graph(_rete.open_bytes(bytes(source)), "<bytes>")
    path = os.fspath(source)
    if isinstance(path, str) and path.startswith(("http://", "https://")):
        if sys.platform == "emscripten":
            # Pyodide: no sockets, so the native HTTP reader is compiled out;
            # remote reads go through the sync-XHR reader instead.
            return Graph(_rete.open_reader(_XhrRangeReader(path, headers)), path)
        return Graph(_rete.open_url(path, dict(headers) if headers else None), path)
    if headers:
        raise TypeError("headers= only applies to http(s) sources")
    return Graph(_rete.open_path(path), path)


# rdflib Datasets label their default graph with this IRI; when serialized to
# N-Quads it must not become a named graph in the .rete file.
_RDFLIB_DEFAULT_GRAPH = "<urn:x-rdflib:default>"


def _as_text_source(source: Any, format: str) -> Tuple[str, str]:
    """Normalize a source to ``(rdf_text, format)``.

    Text passes through with its declared ``format``. A **graph object from
    another RDF library** — anything with a ``.serialize(format=...)`` method
    (duck-typed; no rdflib dependency here) — serializes as N-Triples, or as
    N-Quads when context-aware (rdflib ``Dataset``/``ConjunctiveGraph``) so
    its named graphs survive the round trip.
    """
    if hasattr(source, "serialize") and not isinstance(source, (str, bytes)):
        context_aware = bool(getattr(source, "context_aware", False))
        text = source.serialize(format="nquads" if context_aware else "nt")
        if isinstance(text, bytes):  # rdflib < 6 returned bytes
            text = text.decode("utf-8")
        if context_aware and _RDFLIB_DEFAULT_GRAPH in text:
            # Strip rdflib's synthetic default-graph label so those triples
            # land in the .rete default graph, not a bogus named graph.
            text = text.replace(f" {_RDFLIB_DEFAULT_GRAPH} .", " .")
        return text, ("nq" if context_aware else "nt")
    return source, format


def build(source: Any, format: str = "nt") -> bytes:
    """Build a complete ``.rete`` file image, ready for :func:`open`.

    ``source`` is either RDF **text** (``format`` = ``"nt"``, ``"nq"`` —
    named graphs become a dataset — ``"ttl"``, or ``"rdfxml"``), or a graph
    object from another RDF library (see :func:`_as_text_source`).

    This is the one-shot path with defaults. For step-by-step configuration —
    a Dataset Card, pyramid options, the full-text index — use :class:`Builder`.
    For large datasets prefer the ``rete build`` CLI, which streams,
    compresses harder, and never holds the whole graph in memory.
    """
    text, fmt = _as_text_source(source, format)
    return _rete.build(text, fmt)


_FORMAT_BY_SUFFIX = {
    ".nt": "nt",
    ".nq": "nq",
    ".nquads": "nq",
    ".ttl": "ttl",
    ".turtle": "ttl",
    ".rdf": "rdfxml",
    ".owl": "rdfxml",
    ".xml": "rdfxml",
}


class Builder:
    """Lazily configure a ``.rete`` build, then :meth:`run` and :meth:`export`.

    Every configuration step just records intent and returns ``self`` (so calls
    chain); nothing parses or builds until :meth:`run`. Changing any setting
    after a run invalidates the cached result, and the next :meth:`run`,
    :meth:`export`, or :meth:`graph` rebuilds.

        import rete_graph as rete

        builder = (
            rete.Builder()
            .add_file("people.ttl")
            .add(rdflib_graph)
            .card(title="People", license="CC0-1.0")
            .pyramid(algo="louvain")
            .text_index()
        )
        builder.run()                    # -> bytes; stats in builder.stats
        builder.export("people.rete")    # write the file
        g = builder.graph()              # or query it right away

    See the "Python: build a .rete" tutorial in the docs for the full
    walkthrough (card fields, pyramid trade-offs, verification).
    """

    def __init__(self) -> None:
        self._sources: List[Tuple[str, str]] = []
        self._card: Optional[Dict[str, Any]] = None
        self._pyramid: bool = True
        self._pyramid_algo: str = "louvain"
        self._text_index: bool = False
        self._type_predicate: Optional[str] = None
        self._bytes: Optional[bytes] = None
        #: Build statistics from the last :meth:`run` (``statements``,
        #: ``defaultTriples``, ``namedGraphs``, ``terms``, ``pyramidLevels``).
        self.stats: Optional[Dict[str, Any]] = None

    def _invalidate(self) -> None:
        self._bytes = None
        self.stats = None

    # -- sources ------------------------------------------------------------

    def add(self, source: Any, format: str = "nt") -> "Builder":
        """Queue RDF text (``"nt"``/``"nq"``/``"ttl"``/``"rdfxml"``) or a graph
        object from another RDF library. May be called many times; all sources
        merge into one graph (named graphs from N-Quads sources survive)."""
        self._sources.append(_as_text_source(source, format))
        self._invalidate()
        return self

    def add_file(self, path: Union[str, "os.PathLike[str]"], format: Optional[str] = None) -> "Builder":
        """Queue an RDF file; the format is inferred from the suffix
        (``.nt``, ``.nq``, ``.ttl``, ``.rdf``/``.owl``/``.xml``) unless given."""
        fspath = os.fspath(path)
        fmt = format or _FORMAT_BY_SUFFIX.get(os.path.splitext(fspath)[1].lower())
        if fmt is None:
            raise ValueError(
                f"cannot infer RDF format from {fspath!r}; pass format='nt'|'nq'|'ttl'|'rdfxml'"
            )
        with io.open(fspath, "r", encoding="utf-8") as fh:
            self._sources.append((fh.read(), fmt))
        self._invalidate()
        return self

    # -- the dataset card -----------------------------------------------------

    #: The rete-defined curated card fields settable as bare keywords beyond
    #: the named parameters of :meth:`card`. Anything else lands in the card's
    #: bounded ``extra`` bag — the top level stays rete's namespace, so a
    #: future official field can never collide with a custom one.
    _CARD_FIELDS = frozenset(
        {
            "version",
            "creators",
            "publisher",
            "canonical_url",
            "sparql_endpoint",
            "source_date",
            "derived_from",
            "doi",
            "cite_as",
            "keywords",
            "theme",
        }
    )

    def card(
        self,
        *,
        title: Optional[str] = None,
        description: Optional[str] = None,
        license: Optional[str] = None,
        source: Optional[str] = None,
        created: Optional[str] = None,
        example_queries: Optional[List[str]] = None,
        **extra: Any,
    ) -> "Builder":
        """Set the embedded Dataset Card's curated fields (repeat calls merge).

        Keywords naming a rete-defined field (``version``, ``creators``,
        ``publisher``, ``canonical_url``, ``sparql_endpoint``, ``source_date``,
        ``derived_from``, ``doi``, ``cite_as``, ``keywords`` [free-text tags →
        ``dcat:keyword``/``schema:keywords``], ``theme`` [controlled-vocabulary
        IRIs → ``dcat:theme``]) set that field. **Any other
        keyword is a publisher-defined custom field** and lands in the card's
        ``extra`` bag — pass ``extra={...}`` to merge a whole dict (structured
        values included). The bag is bounded (8 KB serialized, ≤64 keys,
        ≤128-byte keys, nesting ≤2 levels — enforced by the ``rete build``
        CLI; this client writes the card it is given) and folds into the
        file's content hash like every curated field.

        The counts (``triple_count``, ``quad_count``, ``named_graph_count``,
        ``term_count``) and ``format_version`` are filled in automatically at
        build time — read the card back with :meth:`Graph.card` or the
        ``rete card`` CLI.
        """
        fields: Dict[str, Any] = {
            "title": title,
            "description": description,
            "license": license,
            "source": source,
            "created": created,
            "example_queries": example_queries,
        }
        bag: Dict[str, Any] = {}
        for key, value in extra.items():
            if value is None:
                continue
            if key == "extra":
                if not isinstance(value, dict):
                    raise ValueError("extra= must be a dict of custom fields")
                bag.update(value)
            elif key in self._CARD_FIELDS:
                fields[key] = value
            else:
                bag[key] = value
        merged = dict(self._card or {})
        merged.update({k: v for k, v in fields.items() if v is not None})
        if bag:
            existing = dict(merged.get("extra") or {})
            existing.update(bag)
            merged["extra"] = existing
        self._card = merged
        self._invalidate()
        return self

    def example(
        self,
        sparql: str,
        *,
        title: Optional[str] = None,
        question: Optional[str] = None,
        dimension: str = "custom",
        id: Optional[str] = None,
    ) -> "Builder":
        """Attach a runnable example SPARQL query to the Dataset Card.

        Examples travel **inside** the file (the card's rich ``queries``
        library) and are read back by every client — :meth:`Graph.examples`
        here, ``rete card`` in the CLI, the playground's starter queries.
        ``title`` is the short human name, ``question`` the plain-language
        question it answers. May be called many times.

        (For a full auto-generated query library — overview, labels, topology
        tiers derived from the data — build with the ``rete build`` CLI.)
        """
        merged = dict(self._card or {})
        queries = list(merged.get("queries", []))
        n = len(queries) + 1
        # Every field below is REQUIRED by the card schema readers — emitting
        # a partial entry would make `rete card` reject the whole card.
        queries.append(
            {
                "id": id or f"ex-{n}",
                "title": title or f"Example {n}",
                "dimension": dimension,
                "question": question or title or f"Example {n}",
                "sparql": sparql,
                "tier": "index",
                "requires": [],
            }
        )
        merged["queries"] = queries
        self._card = merged
        self._invalidate()
        return self

    # -- build options --------------------------------------------------------

    def pyramid(self, enabled: bool = True, *, algo: str = "louvain") -> "Builder":
        """Configure the community pyramid: ``algo="louvain"`` (topological
        communities, the default) or ``"types"`` (one community per
        ``rdf:type`` class). ``pyramid(False)`` skips it entirely — the file
        stays fully queryable and smaller, but loses the summary/progressive
        views and label prefix search."""
        self._pyramid = enabled
        self._pyramid_algo = algo
        self._invalidate()
        return self

    def text_index(self, enabled: bool = True) -> "Builder":
        """Add the opt-in full-text word index (powers
        :meth:`Graph.text_search` and fast ``CONTAINS`` filters)."""
        self._text_index = enabled
        self._invalidate()
        return self

    def type_predicate(self, predicate_iri: str) -> "Builder":
        """Force the typing predicate (default: auto-detected ``rdf:type``) —
        used by the ``types`` pyramid and the schema profile."""
        self._type_predicate = predicate_iri
        self._invalidate()
        return self

    # -- execution --------------------------------------------------------------

    def run(self) -> bytes:
        """Build (or return the cached) ``.rete`` file image; fills
        :attr:`stats`."""
        if self._bytes is None:
            if not self._sources:
                raise ValueError("add at least one source before run()")
            card_json = json.dumps(self._card) if self._card else None
            data, stats_json = _rete.build_dataset(
                self._sources,
                card_json,
                self._pyramid,
                self._pyramid_algo,
                self._text_index,
                self._type_predicate,
            )
            self._bytes = bytes(data)
            self.stats = json.loads(stats_json)
        return self._bytes

    def export(self, path: Union[str, "os.PathLike[str]"]) -> str:
        """Write the built file to ``path`` (building first if needed) and
        return the path. The file is complete and immutable — host it anywhere
        that serves HTTP ``Range`` and it is queryable in place."""
        fspath = os.fspath(path)
        with io.open(fspath, "wb") as fh:
            fh.write(self.run())
        return fspath

    def graph(self) -> Graph:
        """Open the built image for querying (building first if needed)."""
        return open(self.run())

    def __repr__(self) -> str:
        built = f"built {self.stats['statements']} statements" if self.stats else "not built"
        return f"<rete_graph.Builder: {len(self._sources)} source(s), {built}>"
