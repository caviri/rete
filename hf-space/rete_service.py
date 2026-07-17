"""Core service behind the REST API and the MCP server.

Three responsibilities, all engine-agnostic of transport:

  * **Catalog** — the published datasets (``catalog.json``, exported from the
    playground catalog by ``scripts/export_space_catalog.py``) merged with any
    ``.rete`` files sitting under ``DATA_DIR``.
  * **Lazy graphs with a two-tier cache** — every open is lazy. Remote URLs
    read through :class:`DiskCachedRangeReader`, which persists fetched byte
    blocks on disk (LRU-capped), while the open handle itself caches decoded
    dictionary chunks and index tiles in RAM. Handles live in a small LRU.
  * **Guarded queries** — wall-clock soft timeout, serialized per handle, row
    cap on serialization, and fetch stats echoed so laziness stays visible.
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import threading
import time
from collections import OrderedDict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import httpx
import rete_graph as rete

DATA_DIR = Path(os.environ.get("DATA_DIR") or ("/data" if Path("/data").is_dir() else "data")).resolve()
CACHE_DIR = Path(os.environ.get("RETE_CACHE_DIR") or (DATA_DIR / ".rete-cache")).resolve()
CATALOG_FILE = Path(os.environ.get("CATALOG_FILE") or (Path(__file__).resolve().parent / "catalog.json"))

# Disk-cache block size. Independent of the engine's in-RAM block cache — this
# tier only has to amortize HTTP round trips and survive restarts. 256 KiB
# keeps over-fetch low on small files; contiguous runs still coalesce into
# one ranged GET each, so request counts stay flat on big scans.
CACHE_BLOCK = int(os.environ.get("RETE_CACHE_BLOCK") or (256 << 10))
CACHE_MAX_MB = int(os.environ.get("RETE_CACHE_MAX_MB") or 4096)
MAX_HANDLES = int(os.environ.get("RETE_MAX_HANDLES") or 12)
ROW_CAP = int(os.environ.get("RETE_ROW_CAP") or 10_000)
QUERY_TIMEOUT_S = float(os.environ.get("RETE_QUERY_TIMEOUT_S") or 60)

_http = httpx.Client(follow_redirects=False, timeout=30.0, headers={"User-Agent": "rete-space/1.0"})


# --------------------------------------------------------------------------- #
# Catalog
# --------------------------------------------------------------------------- #

def _local_datasets() -> List[Dict[str, Any]]:
    """Every .rete under DATA_DIR (served by the gateway at /data/<path>)."""
    out = []
    if DATA_DIR.is_dir():
        for p in sorted(DATA_DIR.rglob("*.rete")):
            relative = p.relative_to(DATA_DIR)
            if any(part.startswith(".") for part in relative.parts):
                continue
            out.append({
                "key": f"local/{relative.as_posix()}",
                "label": relative.stem,
                "description": "Local file under this gateway's /data storage.",
                "url": f"/data/{relative.as_posix()}",
                "local_path": str(p),
                "size_bytes": p.stat().st_size,
                "kind": "local",
            })
    return out


def load_catalog() -> List[Dict[str, Any]]:
    """Published datasets + local files. Never raises — an unreadable
    catalog.json degrades to the local listing."""
    published: List[Dict[str, Any]] = []
    try:
        published = json.loads(CATALOG_FILE.read_text(encoding="utf-8"))["datasets"]
    except Exception:
        pass
    return published + _local_datasets()


def find_dataset(key: str) -> Optional[Dict[str, Any]]:
    for d in load_catalog():
        if d["key"] == key:
            return d
    return None


def resolve_source(dataset: Optional[str] = None, url: Optional[str] = None) -> Tuple[str, Dict[str, Any]]:
    """Turn a request's dataset key or raw URL into an openable source.

    Returns ``(source, dataset_entry)`` where source is a URL or local path.
    Raises ValueError with a user-facing message otherwise.
    """
    if url:
        if not url.startswith(("http://", "https://")):
            raise ValueError("url must be http(s); use dataset=local/<file> for local files")
        return url, {"key": url, "url": url, "kind": "url"}
    if not dataset:
        raise ValueError("provide either dataset (a catalog key) or url")
    entry = find_dataset(dataset)
    if entry is None:
        known = ", ".join(sorted(d["key"] for d in load_catalog())[:40])
        raise ValueError(f"unknown dataset {dataset!r}; known keys include: {known}")
    if entry.get("shards"):
        raise ValueError(
            f"dataset {dataset!r} is sharded; query one shard via url= "
            f"(shards: {', '.join(entry['shards'][:3])}…)"
        )
    source = entry.get("local_path") or entry.get("url")
    if not source:
        raise ValueError(f"dataset {dataset!r} has no queryable URL")
    return source, entry


# --------------------------------------------------------------------------- #
# Disk-cached range reader (the persistent tier of the cache)
# --------------------------------------------------------------------------- #

class DiskCachedRangeReader:
    """A rete reader (``read_at``/``len``) that persists fetched blocks.

    Blocks are ``CACHE_BLOCK``-aligned files under ``CACHE_DIR/<urlhash>/``,
    validated against the origin by length + ETag (a republished file wipes
    the entry). Writes are atomic (temp + replace) so concurrent workers can
    share the directory; at worst two workers fetch the same block once.
    """

    def __init__(self, url: str):
        self.url = url
        self.dir = CACHE_DIR / hashlib.sha256(url.encode()).hexdigest()[:24]
        self.network_requests = 0
        self.network_bytes = 0
        self.disk_hits = 0
        self._lock = threading.Lock()
        length, etag = self._probe()
        self._len = length
        self._validate(length, etag)

    def _probe(self) -> Tuple[int, Optional[str]]:
        r = _http.head(self.url)
        if r.status_code == 200 and "content-length" in r.headers:
            return int(r.headers["content-length"]), r.headers.get("etag")
        # Hosts that dislike HEAD: a 1-byte ranged GET carries the total.
        r = _http.get(self.url, headers={"Range": "bytes=0-0"})
        if r.status_code != 206:
            raise IOError(f"host did not answer a Range request with 206 (got {r.status_code}) for {self.url}")
        total = int(r.headers["content-range"].rsplit("/", 1)[1])
        return total, r.headers.get("etag")

    def _validate(self, length: int, etag: Optional[str]) -> None:
        manifest = self.dir / "manifest.json"
        current = {"url": self.url, "length": length, "etag": etag, "block": CACHE_BLOCK}
        try:
            stored = json.loads(manifest.read_text(encoding="utf-8"))
            if (stored.get("length"), stored.get("etag"), stored.get("block")) != (length, etag, CACHE_BLOCK):
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
        """One ranged GET covering blocks [first, last], split into block files."""
        start = first * CACHE_BLOCK
        end = min((last + 1) * CACHE_BLOCK, self._len) - 1
        r = _http.get(self.url, headers={"Range": f"bytes={start}-{end}"})
        if r.status_code != 206:
            raise IOError(f"range request failed ({r.status_code}) for {self.url}")
        body = r.content
        if len(body) != end - start + 1:
            raise IOError(f"short range read ({len(body)} of {end - start + 1} bytes) for {self.url}")
        self.network_requests += 1
        self.network_bytes += len(body)
        for index in range(first, last + 1):
            off = (index - first) * CACHE_BLOCK
            piece = body[off:off + CACHE_BLOCK]
            tmp = self._block_path(index).with_suffix(".tmp")
            tmp.write_bytes(piece)
            os.replace(tmp, self._block_path(index))
        _evict_if_needed()

    def read_at(self, offset: int, length: int) -> bytes:
        if length == 0:
            return b""
        if offset + length > self._len:
            raise IOError(f"read past end of file ({offset}+{length} > {self._len})")
        first = offset // CACHE_BLOCK
        last = (offset + length - 1) // CACHE_BLOCK
        with self._lock:
            # Fetch every missing run of blocks with one request per run.
            run_start = None
            for index in range(first, last + 2):
                missing = index <= last and not self._block_path(index).is_file()
                if missing and run_start is None:
                    run_start = index
                elif not missing and run_start is not None:
                    self._fetch_blocks(run_start, index - 1)
                    run_start = None
            # Assemble.
            parts = []
            for index in range(first, last + 1):
                path = self._block_path(index)
                data = path.read_bytes()
                os.utime(path)  # LRU touch
                parts.append(data)
            self.disk_hits += 1
            blob = b"".join(parts)
        skip = offset - first * CACHE_BLOCK
        return blob[skip:skip + length]


_evict_lock = threading.Lock()


def _evict_if_needed() -> None:
    """Keep the disk cache under CACHE_MAX_MB, evicting least-recently-used
    block files first. Cheap full scan — block counts stay in the thousands."""
    with _evict_lock:
        blocks = [p for p in CACHE_DIR.glob("*/*.bin")]
        total = sum(p.stat().st_size for p in blocks)
        cap = CACHE_MAX_MB * (1 << 20)
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


def cache_overview() -> Dict[str, Any]:
    blocks = list(CACHE_DIR.glob("*/*.bin")) if CACHE_DIR.is_dir() else []
    return {
        "cache_dir": str(CACHE_DIR),
        "entries": len(list(CACHE_DIR.glob("*/manifest.json"))) if CACHE_DIR.is_dir() else 0,
        "blocks": len(blocks),
        "bytes": sum(p.stat().st_size for p in blocks),
        "cap_bytes": CACHE_MAX_MB * (1 << 20),
        "block_size": CACHE_BLOCK,
        "open_handles": list(_handles.keys()),
    }


# --------------------------------------------------------------------------- #
# Graph handle LRU (the RAM tier)
# --------------------------------------------------------------------------- #

@dataclass
class Handle:
    graph: Any
    source: str
    reader: Optional[DiskCachedRangeReader]
    lock: threading.Lock = field(default_factory=threading.Lock)
    opened_at: float = field(default_factory=time.time)


_handles: "OrderedDict[str, Handle]" = OrderedDict()
_handles_lock = threading.Lock()


def get_handle(source: str) -> Handle:
    """An open, resident graph for a URL or local path (LRU of MAX_HANDLES)."""
    with _handles_lock:
        if source in _handles:
            _handles.move_to_end(source)
            return _handles[source]
    # Open outside the registry lock — opens can take a second on cold cache.
    if source.startswith(("http://", "https://")):
        reader = DiskCachedRangeReader(source)
        graph = rete.open(reader=reader)
    else:
        reader = None
        graph = rete.open(source)
    handle = Handle(graph=graph, source=source, reader=reader)
    with _handles_lock:
        if source in _handles:  # lost a race; keep the first
            return _handles[source]
        _handles[source] = handle
        while len(_handles) > MAX_HANDLES:
            _handles.popitem(last=False)
        return handle


def handle_stats(handle: Handle) -> Dict[str, Any]:
    stats = handle.graph.stats()
    if handle.reader is not None:
        stats["network_requests"] = handle.reader.network_requests
        stats["network_bytes"] = handle.reader.network_bytes
        stats["disk_cache_reads"] = handle.reader.disk_hits
    return stats


# --------------------------------------------------------------------------- #
# Queries
# --------------------------------------------------------------------------- #

def _bindings(env: Dict[str, Any]) -> Tuple[List[str], List[Dict[str, Any]], List[Dict[str, Any]]]:
    """W3C SPARQL-JSON bindings + a plain-Python table from a SELECT envelope."""
    variables = list(env.get("vars") or [])
    bindings, table = [], []
    for row in env.get("rows") or []:
        w3c, plain = {}, {}
        for var in variables:
            token = row.get(var)
            if token is None:
                continue
            term = rete.Term.parse(token)
            if term.kind == "iri":
                w3c[var] = {"type": "uri", "value": term.value}
            elif term.kind == "bnode":
                w3c[var] = {"type": "bnode", "value": term.value.lstrip("_:")}
            else:
                entry = {"type": "literal", "value": term.value}
                if term.datatype:
                    entry["datatype"] = term.datatype
                if term.lang:
                    entry["xml:lang"] = term.lang
                w3c[var] = entry
            plain[var] = term.to_python()
        bindings.append(w3c)
        table.append(plain)
    return variables, bindings, table


def run_query(dataset: Optional[str], url: Optional[str], query: str,
              reason: bool = False, row_cap: Optional[int] = None) -> Dict[str, Any]:
    """Run a SPARQL query and return a transport-ready result document.

    SELECT → W3C SPARQL-JSON (`head`/`results`) plus a `table` of plain
    values; ASK → `boolean`; CONSTRUCT/DESCRIBE → `triples` (token form).
    Every response carries `stats` — the bytes physically fetched.
    """
    source, entry = resolve_source(dataset, url)
    cap = min(row_cap or ROW_CAP, ROW_CAP)
    handle = get_handle(source)
    started = time.time()
    with handle.lock:
        env = handle.graph.query_raw(query, reason=reason)
    elapsed = time.time() - started
    if elapsed > QUERY_TIMEOUT_S:
        raise TimeoutError(f"query took {elapsed:.1f}s (limit {QUERY_TIMEOUT_S:.0f}s)")

    doc: Dict[str, Any] = {
        "dataset": entry.get("key"),
        "kind": env.get("kind"),
        "elapsed_seconds": round(elapsed, 3),
        "stats": handle_stats(handle),
    }
    if env.get("kind") == "ask":
        doc["boolean"] = bool(env.get("boolean"))
    elif env.get("kind") == "construct":
        triples = env.get("triples") or []
        doc["truncated"] = len(triples) > cap
        doc["triples"] = triples[:cap]
    else:
        variables, bindings, table = _bindings(env)
        doc["truncated"] = len(bindings) > cap
        doc["head"] = {"vars": variables}
        doc["results"] = {"bindings": bindings[:cap]}
        doc["table"] = table[:cap]
    return doc


def dataset_card(dataset: str) -> Optional[Dict[str, Any]]:
    source, _ = resolve_source(dataset, None)
    handle = get_handle(source)
    with handle.lock:
        return handle.graph.card()


def dataset_schema(dataset: str) -> Dict[str, Any]:
    source, _ = resolve_source(dataset, None)
    handle = get_handle(source)
    with handle.lock:
        return {"info": handle.graph.info(), "schema": handle.graph.schema(),
                "graphs": handle.graph.graph_names(), "stats": handle_stats(handle)}


def dataset_examples(dataset: str) -> List[Dict[str, Any]]:
    """Example queries: the file's embedded ones, else the catalog's."""
    source, entry = resolve_source(dataset, None)
    handle = get_handle(source)
    with handle.lock:
        embedded = handle.graph.examples()
    if embedded:
        return embedded
    return entry.get("examples") or []


def entity_search(text: str, dataset: str, limit: int = 20) -> List[Dict[str, Any]]:
    """Label prefix search plus full-text (when the file has a text index)."""
    source, _ = resolve_source(dataset, None)
    handle = get_handle(source)
    out, seen = [], set()
    with handle.lock:
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


def describe_entity(dataset: str, iri: str, cap: int = 200) -> Dict[str, Any]:
    doc = run_query(dataset, None, f"DESCRIBE <{iri}>", row_cap=cap)
    return doc
