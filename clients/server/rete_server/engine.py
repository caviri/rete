"""Graph handles and guarded queries — the part every route shares.

* **Handles.** Each dataset is opened once, lazily (HTTP Range for remote
  files, positional reads for local ones), and stays resident in an LRU of
  ``max_handles``. A resident handle keeps decoded dictionary chunks and
  index tiles in RAM, so a warm query reads nothing. Opens are single-flight:
  a burst of first queries on one dataset opens it once, not once per request.
* **Two readers.** By default remote files go through the engine's own HTTP
  reader (Rust, no Python per read). With ``cache_dir`` set they go through
  :class:`DiskCachedRangeReader` instead, which keeps fetched blocks on disk —
  on a volume they survive restarts, so a redeployed server answers warm.
* **Guards.** A row cap on what is serialized, a wall-clock limit reported as
  504, and a per-handle lock taken with a timeout (busy → 503, never a pile of
  blocked threads).

Why queries on one handle still take turns: the engine resets a handle's
"a range fetch failed" verdict at the start of every query, so two queries
running at once on one handle could clear each other's failure and return an
incomplete answer as if it were complete. Until that verdict is per query,
the lock is what keeps the "never silently incomplete" guarantee.
"""
from __future__ import annotations

import hashlib
import ipaddress
import json
import logging
import os
import re
import shutil
import socket
import threading
import time
from collections import OrderedDict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple
from urllib.parse import urlsplit

import httpx
import rete_graph as rete

from .catalog import Catalog
from .settings import Settings

log = logging.getLogger("rete_server.engine")


# --------------------------------------------------------------------------- #
# Errors (the routes map them to status codes)
# --------------------------------------------------------------------------- #

# Where Claude's own sandbox keeps the files it works with: a file attached to
# the chat is /mnt/user-data/uploads/<name> there, out of this server's reach.
SANDBOX = re.compile(r"^/(mnt/user-data|mnt/data|home/claude)/")


class RequestError(ValueError):
    """The request is wrong: 400."""


class UnknownDataset(RequestError):
    """No such catalog key: 404."""


class UnrepresentableResult(RequestError):
    """A CONSTRUCT result the requested quoted-triple surface cannot spell."""


class Busy(RuntimeError):
    """The dataset is serving another query and did not free up in time: 503."""


class QueryTimeout(TimeoutError):
    """The query ran past the limit: 504."""


QUOTED_TRIPLE_SYNTAXES = ("rdf12", "rdf-star")


# --------------------------------------------------------------------------- #
# URL policy
# --------------------------------------------------------------------------- #

def check_public_url(url: str) -> None:
    """Refuse a ``url=`` that points into this machine or its private network.

    A server that range-reads any URL a client names would otherwise let that
    client probe the loopback interface, the container network or a cloud
    metadata address. Catalog entries are trusted and never pass through here.
    """
    parts = urlsplit(url)
    if parts.scheme not in ("http", "https") or not parts.hostname:
        raise RequestError("url must be an absolute http(s) URL")
    try:
        infos = socket.getaddrinfo(parts.hostname, parts.port or 443, proto=socket.IPPROTO_TCP)
    except socket.gaierror as e:
        raise RequestError(f"cannot resolve {parts.hostname}: {e}") from None
    for info in infos:
        ip = ipaddress.ip_address(info[4][0])
        if not ip.is_global:
            raise RequestError(
                f"url resolves to a non-public address ({ip}); this server only reads public "
                "URLs (RETE_ALLOW_PRIVATE_URLS=1 lifts that)")


# --------------------------------------------------------------------------- #
# Disk-cached range reader (opt-in persistent tier)
# --------------------------------------------------------------------------- #

class DiskCachedRangeReader:
    """A rete reader (``read_at``/``len``) that persists fetched blocks.

    Blocks are ``block``-aligned files under ``<cache_dir>/<urlhash>/``,
    validated against the origin by length + ETag at open (a republished file
    wipes its entry). Writes are atomic (temp + replace). Contiguous missing
    blocks are fetched with one ranged GET.
    """

    def __init__(self, url: str, cache_dir: Path, max_bytes: int, http: httpx.Client,
                 block: int = 256 << 10):
        self.url = url
        self.block = block
        self.cache_dir = cache_dir
        self.max_bytes = max_bytes
        self.dir = cache_dir / hashlib.sha256(url.encode()).hexdigest()[:24]
        self.network_requests = 0
        self.network_bytes = 0
        self.disk_hits = 0
        self._http = http
        self._lock = threading.Lock()
        length, etag = self._probe()
        self._len = length
        self._validate(length, etag)

    def _probe(self) -> Tuple[int, Optional[str]]:
        r = self._http.head(self.url)
        if r.status_code == 200 and "content-length" in r.headers:
            return int(r.headers["content-length"]), r.headers.get("etag")
        r = self._http.get(self.url, headers={"Range": "bytes=0-0"})
        if r.status_code != 206:
            raise IOError(f"host did not answer a Range request with 206 (got {r.status_code}) for {self.url}")
        return int(r.headers["content-range"].rsplit("/", 1)[1]), r.headers.get("etag")

    def _validate(self, length: int, etag: Optional[str]) -> None:
        manifest = self.dir / "manifest.json"
        current = {"url": self.url, "length": length, "etag": etag, "block": self.block}
        try:
            stored = json.loads(manifest.read_text(encoding="utf-8"))
            if (stored.get("length"), stored.get("etag"), stored.get("block")) != (length, etag, self.block):
                shutil.rmtree(self.dir, ignore_errors=True)
        except FileNotFoundError:
            pass
        except Exception:
            shutil.rmtree(self.dir, ignore_errors=True)
        self.dir.mkdir(parents=True, exist_ok=True)
        tmp = manifest.with_suffix(".tmp")
        tmp.write_text(json.dumps(current), encoding="utf-8")
        os.replace(tmp, manifest)

    def len(self) -> int:
        return self._len

    def _block_path(self, index: int) -> Path:
        return self.dir / f"{index:09d}.bin"

    def _fetch_blocks(self, first: int, last: int) -> None:
        start = first * self.block
        end = min((last + 1) * self.block, self._len) - 1
        r = self._http.get(self.url, headers={"Range": f"bytes={start}-{end}"})
        if r.status_code != 206:
            raise IOError(f"range request failed ({r.status_code}) for {self.url}")
        body = r.content
        if len(body) != end - start + 1:
            raise IOError(f"short range read ({len(body)} of {end - start + 1} bytes) for {self.url}")
        self.network_requests += 1
        self.network_bytes += len(body)
        for index in range(first, last + 1):
            off = (index - first) * self.block
            tmp = self._block_path(index).with_suffix(".tmp")
            tmp.write_bytes(body[off:off + self.block])
            os.replace(tmp, self._block_path(index))
        _evict(self.cache_dir, self.max_bytes)

    def read_at(self, offset: int, length: int) -> bytes:
        if length == 0:
            return b""
        if offset + length > self._len:
            raise IOError(f"read past end of file ({offset}+{length} > {self._len})")
        first, last = offset // self.block, (offset + length - 1) // self.block
        with self._lock:
            run_start = None
            for index in range(first, last + 2):
                missing = index <= last and not self._block_path(index).is_file()
                if missing and run_start is None:
                    run_start = index
                elif not missing and run_start is not None:
                    self._fetch_blocks(run_start, index - 1)
                    run_start = None
            parts = []
            for index in range(first, last + 1):
                path = self._block_path(index)
                parts.append(path.read_bytes())
                os.utime(path)  # LRU touch
            self.disk_hits += 1
        skip = offset - first * self.block
        return b"".join(parts)[skip:skip + length]


_evict_lock = threading.Lock()


def _evict(cache_dir: Path, cap: int) -> None:
    """Keep the disk cache under ``cap`` bytes, least recently used first."""
    with _evict_lock:
        blocks = list(cache_dir.glob("*/*.bin"))
        total = sum(p.stat().st_size for p in blocks)
        if total <= cap:
            return
        for p in sorted(blocks, key=lambda p: p.stat().st_mtime):
            try:
                total -= p.stat().st_size
                p.unlink()
            except OSError:
                continue
            if total <= cap:
                return


# --------------------------------------------------------------------------- #
# Result shaping
# --------------------------------------------------------------------------- #

_XSD = "http://www.w3.org/2001/XMLSchema#"


def _term(token: str) -> Tuple[Dict[str, Any], Any]:
    """One engine token → (W3C SPARQL-JSON term, plain Python value)."""
    if token.startswith("<") and not token.startswith("<<") and "\\" not in token:
        iri = token[1:-1]  # the common case, without the general parser
        return {"type": "uri", "value": iri}, iri
    term = rete.Term.parse(token)
    if term.kind == "iri":
        return {"type": "uri", "value": term.value}, term.value
    if term.kind == "bnode":
        label = term.value[2:] if term.value.startswith("_:") else term.value
        return {"type": "bnode", "value": label}, term.value
    entry = {"type": "literal", "value": term.value}
    if term.datatype:
        entry["datatype"] = term.datatype
    if term.lang:
        entry["xml:lang"] = term.lang
    return entry, term.to_python()


SHAPES = ("both", "w3c", "table")


def _bindings(env: Dict[str, Any], cap: int, shape: str = "both"
              ) -> Tuple[List[str], Optional[List[Dict[str, Any]]], Optional[List[Dict[str, Any]]], bool]:
    """SELECT rows as W3C bindings and/or plain values — only what is asked
    for. A result repeats its terms (predicates, classes, hub IRIs), so each
    distinct token is converted once."""
    variables = list(env.get("vars") or [])
    rows = env.get("rows") or []
    want_w3c, want_plain = shape in ("both", "w3c"), shape in ("both", "table")
    seen: Dict[str, Tuple[Dict[str, Any], Any]] = {}
    bindings: List[Dict[str, Any]] = []
    table: List[Dict[str, Any]] = []
    for row in rows[:cap]:
        w3c, plain = {}, {}
        for var in variables:
            token = row.get(var)
            if token is None:
                continue
            term = seen.get(token)
            if term is None:
                term = seen[token] = _term(token)
            if want_w3c:
                w3c[var] = term[0]
            if want_plain:
                plain[var] = term[1]
        if want_w3c:
            bindings.append(w3c)
        if want_plain:
            table.append(plain)
    return (variables, bindings if want_w3c else None, table if want_plain else None,
            len(rows) > cap)


def respell_triples(triples: List[List[str]], quoted_triple_syntax: str) -> List[List[str]]:
    """Write quoted triples the way ``rete export`` does by default.

    The engine returns CONSTRUCT/DESCRIBE triples as stored tokens, where a
    quoted triple is RDF-star ``<<s p o>>``. RDF 1.2 parsers reject that in
    N-Triples and read it as a reifier in Turtle, so ``"rdf12"`` rewrites it to
    the triple term ``<<( s p o )>>``. RDF 1.2 has no spelling for a quoted
    triple in subject or predicate position, so that is refused by name.
    """
    if quoted_triple_syntax not in QUOTED_TRIPLE_SYNTAXES:
        raise RequestError(f"unknown quoted_triple_syntax {quoted_triple_syntax!r} "
                           "(expected 'rdf12' or 'rdf-star')")
    if quoted_triple_syntax == "rdf-star":
        return triples
    out = []
    for triple in triples:
        s, p, o = triple[0], triple[1], triple[2]
        if not (s.startswith("<<") or p.startswith("<<") or o.startswith("<<")):
            out.append(triple)
            continue
        try:
            # The Python client's RDF 1.2 writer (rete_core::terms::rdf12_triple_term),
            # so a triple term is spelled byte for byte as the CLI export spells it.
            o = rete._rdf12_object(s, p, o)
        except ValueError as e:
            raise UnrepresentableResult(str(e)) from None
        out.append([s, p, o, *triple[3:]])
    return out


# --------------------------------------------------------------------------- #
# The service
# --------------------------------------------------------------------------- #

@dataclass
class Handle:
    graph: Any
    source: str
    reader: Optional[DiskCachedRangeReader]
    lock: threading.Lock = field(default_factory=threading.Lock)
    lane: str = "quick"
    opened_at: float = field(default_factory=time.time)
    queries: int = 0


class Service:
    def __init__(self, settings: Settings, catalog: Catalog):
        self.settings = settings
        self.catalog = catalog
        self._http = httpx.Client(follow_redirects=False, timeout=30.0,
                                  headers={"User-Agent": "rete-server"})
        self._handles: "OrderedDict[str, Handle]" = OrderedDict()
        self._handles_lock = threading.Lock()
        self._opening: Dict[str, threading.Lock] = {}

    # -- sources ---------------------------------------------------------

    def resolve(self, dataset: Optional[str] = None, url: Optional[str] = None) -> Tuple[str, Dict[str, Any]]:
        """A request's dataset key or URL → ``(source, entry)``."""
        if url:
            if not self.settings.allow_urls:
                raise RequestError("this server only answers for its catalog; pass dataset=<key>")
            if not self.settings.allow_private_urls:
                check_public_url(url)
            elif not url.startswith(("http://", "https://")):
                raise RequestError("url must be http(s); local files are dataset=local/<path>")
            return url, {"key": url, "url": url, "kind": "url"}
        if not dataset:
            raise RequestError("provide either dataset (a catalog key) or url")
        if SANDBOX.match(dataset):
            if self.settings.author:
                how = (f'To open it, call graph_view with dataset "{dataset}": the view asks the user to '
                       "choose or drop the file, which then becomes a dataset here (the view says its "
                       "generated/… key).")
            else:
                how = "This server takes no files from users (authoring is off)."
            raise RequestError(f"{dataset} is a file attached to the chat: Claude's copy of it is in Claude's "
                               f"own sandbox, out of this server's reach. {how}")
        entry = self.catalog.get(dataset)
        if entry is None:
            known = ", ".join(sorted(self.catalog.keys())[:40])
            raise UnknownDataset(f"unknown dataset {dataset!r}; known keys include: {known}")
        if entry.get("shards"):
            raise RequestError(f"dataset {dataset!r} is sharded; query one shard via url= "
                               f"(shards: {', '.join(entry['shards'][:3])}…)")
        source = entry.get("local_path") or entry.get("url")
        if not source:
            raise RequestError(f"dataset {dataset!r} has no queryable URL")
        return source, entry

    # -- handles ---------------------------------------------------------

    def handle(self, source: str, lane: str = "quick") -> Handle:
        """The open graph for ``source``. Long work (MCP tasks) uses lane
        ``"long"``: a second handle of the same file with its own lock, so a
        query that runs for minutes never blocks a quick call on the dataset."""
        key = source if lane == "quick" else f"{source}#{lane}"
        with self._handles_lock:
            if key in self._handles:
                self._handles.move_to_end(key)
                return self._handles[key]
            opening = self._opening.setdefault(key, threading.Lock())
        # Single flight: one open per source, the others wait for it.
        with opening:
            with self._handles_lock:
                if key in self._handles:
                    self._handles.move_to_end(key)
                    return self._handles[key]
            handle = self._open(source)
            handle.lane = lane
            with self._handles_lock:
                self._handles[key] = handle
                self._opening.pop(key, None)
                while len(self._handles) > self.settings.max_handles:
                    self._handles.popitem(last=False)
                return handle

    def _open(self, source: str) -> Handle:
        started = time.time()
        reader = None
        if source.startswith(("http://", "https://")):
            if self.settings.cache_dir is not None:
                reader = DiskCachedRangeReader(
                    source, self.settings.cache_dir, self.settings.cache_max_mb << 20, self._http)
                graph = rete.open(reader=reader)
            else:
                graph = rete.open(source)
        else:
            graph = rete.open(source)
        log.info("opened %s in %.2fs", source, time.time() - started)
        return Handle(graph=graph, source=source, reader=reader)

    def _limit(self, lane: str) -> float:
        """The time limit of a lane: quick calls, or long work (a task)."""
        return self.settings.task_timeout_s if lane == "long" else self.settings.query_timeout_s

    def _locked(self, handle: Handle):
        """The handle's lock, or Busy when it does not free up in time."""
        wait = self._limit(handle.lane)
        if not handle.lock.acquire(timeout=wait):
            if handle.lane == "long":
                raise Busy(f"{handle.source} has been busy with another long query for {wait:.0f}s; "
                           "retry when it has finished")
            raise Busy(f"{handle.source} is busy with another query; retry shortly")
        return _Release(handle.lock)

    def stats(self, handle: Handle) -> Dict[str, Any]:
        stats = handle.graph.stats()
        stats.pop("schemaVersion", None)
        if handle.reader is not None:
            stats["network_requests"] = handle.reader.network_requests
            stats["network_bytes"] = handle.reader.network_bytes
            stats["disk_cache_reads"] = handle.reader.disk_hits
        return stats

    def warm(self, keys: List[str]) -> None:
        """Open these datasets now (and read their cards) so the first query
        skips the open. Errors are logged, never raised."""
        for key in keys:
            try:
                handle = self.handle(self.resolve(key)[0])
                with self._locked(handle):
                    handle.graph.card()
            except Exception as e:
                log.warning("warm %s: %s", key, e)

    def overview(self) -> Dict[str, Any]:
        with self._handles_lock:
            handles = [{"source": h.source, "opened_at": round(h.opened_at), "queries": h.queries,
                        "busy": h.lock.locked()} for h in self._handles.values()]
        doc: Dict[str, Any] = {"open_handles": handles, "max_handles": self.settings.max_handles}
        cache = self.settings.cache_dir
        if cache is not None:
            blocks = list(cache.glob("*/*.bin")) if cache.is_dir() else []
            doc["disk_cache"] = {"dir": str(cache), "blocks": len(blocks),
                                 "bytes": sum(p.stat().st_size for p in blocks),
                                 "cap_bytes": self.settings.cache_max_mb << 20}
        return doc

    # -- queries ---------------------------------------------------------

    def run_query(self, dataset: Optional[str], url: Optional[str], query: str,
                  reason: bool = False, row_cap: Optional[int] = None,
                  quoted_triple_syntax: str = "rdf12", shape: str = "both",
                  lane: str = "quick") -> Dict[str, Any]:
        """Run SPARQL and return a transport-ready document.

        SELECT → W3C SPARQL-JSON (``head``/``results``) plus ``table``, the
        same rows as plain values; ASK → ``boolean``; CONSTRUCT/DESCRIBE →
        ``triples``. Every answer carries ``stats`` (bytes physically fetched)
        and ``warnings``: the FILTER type errors the engine reported, which
        explain rows that vanished without an error. ``shape`` picks the
        SELECT forms to build: ``both``, ``w3c`` (bindings only, what the
        SPARQL Protocol sends) or ``table``.
        """
        if shape not in SHAPES:
            raise RequestError(f"unknown shape {shape!r} (expected one of {', '.join(SHAPES)})")
        if quoted_triple_syntax not in QUOTED_TRIPLE_SYNTAXES:
            raise RequestError(f"unknown quoted_triple_syntax {quoted_triple_syntax!r} "
                               "(expected 'rdf12' or 'rdf-star')")
        source, entry = self.resolve(dataset, url)
        ceiling = self.settings.task_row_cap if lane == "long" else self.settings.row_cap
        cap = min(row_cap or ceiling, ceiling)
        limit = self._limit(lane)
        handle = self.handle(source, lane)
        with self._locked(handle):
            started = time.time()
            try:
                env = handle.graph.query_raw(query, reason=reason)
            except ValueError as e:  # parse / evaluation errors from the engine
                raise RequestError(str(e)) from None
            elapsed = time.time() - started
            handle.queries += 1
            stats = self.stats(handle)
        if elapsed > limit:
            raise QueryTimeout(f"query took {elapsed:.1f}s (limit {limit:.0f}s); "
                               "narrow it or add a LIMIT")
        doc: Dict[str, Any] = {
            "dataset": entry.get("key"),
            "kind": env.get("kind"),
            "elapsed_seconds": round(elapsed, 3),
            "stats": stats,
            # Per call: the envelope carries this query's own warnings.
            "warnings": env.get("warnings") or [],
        }
        if env.get("kind") == "ask":
            doc["boolean"] = bool(env.get("boolean"))
        elif env.get("kind") == "construct":
            triples = env.get("triples") or []
            doc["truncated"] = len(triples) > cap
            doc["triples"] = respell_triples(triples[:cap], quoted_triple_syntax)
        else:
            variables, bindings, table, truncated = _bindings(env, cap, shape)
            doc["truncated"] = truncated
            doc["head"] = {"vars": variables}
            if bindings is not None:
                doc["results"] = {"bindings": bindings}
            if table is not None:
                doc["table"] = table
        return doc

    def source_of(self, name: str) -> Tuple[str, Dict[str, Any]]:
        """A dataset key, or a public ``.rete`` URL, → ``(source, entry)``."""
        if name.startswith(("http://", "https://")):
            return self.resolve(url=name)
        return self.resolve(dataset=name)

    def query_env(self, name: str, query: str) -> Dict[str, Any]:
        """The engine's own answer, term tokens and all, for a caller that
        reads it itself (the graph view). The same lock and time limit as
        :meth:`run_query`."""
        source, _ = self.source_of(name)
        handle = self.handle(source)
        with self._locked(handle):
            started = time.time()
            try:
                env = handle.graph.query_raw(query, reason=False)
            except ValueError as e:
                raise RequestError(str(e)) from None
            elapsed = time.time() - started
            handle.queries += 1
        if elapsed > self.settings.query_timeout_s:
            raise QueryTimeout(f"query took {elapsed:.1f}s (limit {self.settings.query_timeout_s:.0f}s); "
                               "narrow it or add a LIMIT")
        return env

    def schema_lists(self, name: str) -> Dict[str, Any]:
        """Classes ``[iri, n]`` and class-level relations ``[s, p, o, n]``:
        from the schema pyramid, or else the lists the card carries (a file
        built without a pyramid). For the graph view's schema diagram."""
        source, entry = self.source_of(name)
        handle = self.handle(source)
        with self._locked(handle):
            try:
                schema = handle.graph.schema()
            except Exception:
                schema = None
            card = None if schema else (handle.graph.card() or {})
        if schema:
            return {"classes": schema.get("classes") or [], "relations": schema.get("relations") or [],
                    "from": "pyramid"}

        def bare(term: Any) -> str:
            s = str(term)
            return s[1:-1] if s.startswith("<") and s.endswith(">") else s

        classes = [[bare(c), n] for c, n in card.get("classes") or []]
        relations = [[bare(link.get("s_class")), bare(link.get("predicate")), bare(link.get("o_class")),
                      link.get("count") or 0] for link in card.get("class_links") or []]
        if not classes:
            raise RequestError(f"{entry.get('key')} has no schema pyramid, and its card lists no classes: "
                               "there is no schema to draw")
        return {"classes": classes, "relations": relations, "from": "card"}

    def ask_boolean(self, dataset: Optional[str], url: Optional[str], query: str) -> bool:
        return bool(self.run_query(dataset, url, query).get("boolean"))

    def dataset_card(self, dataset: str) -> Optional[Dict[str, Any]]:
        handle = self.handle(self.resolve(dataset)[0])
        with self._locked(handle):
            return handle.graph.card()

    def dataset_schema(self, dataset: str) -> Dict[str, Any]:
        handle = self.handle(self.resolve(dataset)[0])
        with self._locked(handle):
            return {"info": handle.graph.info(), "schema": handle.graph.schema(),
                    "graphs": handle.graph.graph_names(), "stats": self.stats(handle)}

    def dataset_examples(self, dataset: str) -> List[Dict[str, Any]]:
        """The file's embedded example queries, else the catalog's."""
        source, entry = self.resolve(dataset)
        handle = self.handle(source)
        with self._locked(handle):
            embedded = handle.graph.examples()
        return embedded or entry.get("examples") or []

    def shacl_validate(self, dataset: Optional[str], url: Optional[str], shapes: str,
                       graph: Optional[str] = None, format: str = "json",
                       lane: str = "quick") -> Dict[str, Any]:
        source, entry = self.resolve(dataset, url)
        handle = self.handle(source, lane)
        with self._locked(handle):
            started = time.time()
            try:
                out = handle.graph.shacl(shapes, graph=graph, format=format)
            except ValueError as e:
                raise RequestError(str(e)) from None
            elapsed = time.time() - started
            stats = self.stats(handle)
        if lane == "long" and elapsed > self._limit(lane):
            raise QueryTimeout(f"validation took {elapsed:.1f}s (limit {self._limit(lane):.0f}s)")
        return {"dataset": entry.get("key"), "elapsed_seconds": round(elapsed, 3),
                "report": out, "stats": stats}

    def shacl_shapes(self, dataset: str) -> List[Dict[str, Any]]:
        return self.resolve(dataset)[1].get("shacl_shapes") or []

    def entity_search(self, text: str, dataset: str, limit: int = 20) -> List[Dict[str, Any]]:
        """Label prefix search, plus full-text when the file has a text index."""
        handle = self.handle(self.resolve(dataset)[0])
        out, seen = [], set()
        with self._locked(handle):
            for label, subject in handle.graph.prefix_search(text, limit):
                if subject not in seen:
                    seen.add(subject)
                    out.append({"subject": subject, "label": label, "via": "label-prefix"})
            try:
                for subject in handle.graph.text_search(text.split(), limit=limit):
                    if subject not in seen:
                        seen.add(subject)
                        out.append({"subject": subject, "label": None, "via": "text-index"})
            except Exception:
                pass  # no text index in this file
        return out[:limit]

    def describe_entity(self, dataset: str, iri: str, cap: int = 200) -> Dict[str, Any]:
        if any(c in iri for c in "<> \"{}|\\^`\n"):
            raise RequestError("iri must be a bare IRI without <>, spaces or quotes")
        return self.run_query(dataset, None, f"DESCRIBE <{iri}>", row_cap=cap)


class _Release:
    def __init__(self, lock: threading.Lock):
        self._lock = lock

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self._lock.release()
        return False
