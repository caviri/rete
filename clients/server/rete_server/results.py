"""Stored answers: big results that stay out of the model's context.

A query answer with thousands of rows is no use inline: the client either
dumps it into the conversation or cuts it. ``sparql_query`` therefore keeps a
big answer here and returns a summary plus a handle; the rest is read in pages
(``result_rows`` / ``rete://results/<id>``) or downloaded
(``GET /api/results/<id>?format=csv``).

One file per answer, ``<id>.jsonl``: the first line is the summary (columns,
counts, stats), every other line is one row. Paging skips lines, so a page far
into a big answer costs a scan of the file, not a parse of it. Files are
written to a temp name and renamed, so a reader never sees half an answer.
The store is capped in total size (oldest evicted first) and every answer
expires after a time to live.
"""
from __future__ import annotations

import csv
import io
import os
import re
import secrets
import threading
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Tuple

import orjson
from fastapi import APIRouter, HTTPException
from fastapi.responses import StreamingResponse

from . import state

URI_PREFIX = "rete://results/"
_ID = re.compile(r"^[A-Za-z0-9_-]{16,64}$")
FORMATS = ("json", "srj", "csv", "tsv", "nt")


class ResultGone(LookupError):
    """The id is unknown, malformed or past its time to live."""


class ResultStore:
    def __init__(self, directory: Path, max_bytes: int, ttl_s: float):
        self.dir = Path(directory)
        self.max_bytes = max_bytes
        self.ttl_s = ttl_s
        self._lock = threading.Lock()

    # -- paths -----------------------------------------------------------

    def _path(self, rid: str) -> Path:
        if not _ID.match(rid or ""):
            raise ResultGone(f"result {rid!r} is unknown or has expired; run the query again")
        return self.dir / f"{rid}.jsonl"

    @staticmethod
    def parse_ref(ref: str) -> str:
        """A result id, or its ``rete://results/<id>`` URI, to the id."""
        ref = (ref or "").strip()
        if ref.startswith(URI_PREFIX):
            ref = ref[len(URI_PREFIX):]
        return ref.split("?", 1)[0].strip("/")

    # -- writing ---------------------------------------------------------

    def put(self, summary: Dict[str, Any], rows: List[Any]) -> Dict[str, Any]:
        """Store one answer; returns its handle (id, uri, expiry, row count)."""
        self.dir.mkdir(parents=True, exist_ok=True)
        rid = secrets.token_urlsafe(24)
        now = time.time()
        head = dict(summary, id=rid, created=now, rows=len(rows))
        tmp = self.dir / f"{rid}.tmp"
        with open(tmp, "wb") as f:
            f.write(orjson.dumps(head) + b"\n")
            for row in rows:
                f.write(orjson.dumps(row) + b"\n")
        os.replace(tmp, self._path(rid))
        self.evict()
        return self.handle(head)

    def handle(self, head: Dict[str, Any]) -> Dict[str, Any]:
        rid = head["id"]
        kinds = ("json", "srj", "csv", "tsv") if head.get("kind") == "select" else ("json", "nt")
        return {
            "id": rid,
            "uri": URI_PREFIX + rid,
            "rows": head["rows"],
            "expires": datetime.fromtimestamp(head["created"] + self.ttl_s, timezone.utc)
                               .strftime("%Y-%m-%dT%H:%M:%SZ"),
            "download": {k: f"/api/results/{rid}?format={k}" for k in kinds},
        }

    def evict(self) -> None:
        """Drop expired answers, then the oldest until the store fits its cap."""
        with self._lock:
            now = time.time()
            files = []
            for p in self.dir.glob("*.jsonl"):
                try:
                    st = p.stat()
                except OSError:
                    continue
                if now - st.st_mtime > self.ttl_s:
                    p.unlink(missing_ok=True)
                else:
                    files.append((st.st_mtime, st.st_size, p))
            for p in self.dir.glob("*.tmp"):  # a crashed writer
                try:
                    if now - p.stat().st_mtime > 600:
                        p.unlink(missing_ok=True)
                except OSError:
                    pass
            total = sum(size for _, size, _ in files)
            for _, size, p in sorted(files):
                if total <= self.max_bytes:
                    break
                p.unlink(missing_ok=True)
                total -= size

    # -- reading ---------------------------------------------------------

    def head(self, rid: str) -> Dict[str, Any]:
        path = self._path(rid)
        try:
            with open(path, "rb") as f:
                head = orjson.loads(f.readline())
        except (FileNotFoundError, orjson.JSONDecodeError):
            raise ResultGone(f"result {rid} is unknown or has expired; run the query again") from None
        if time.time() - head["created"] > self.ttl_s:
            path.unlink(missing_ok=True)
            raise ResultGone(f"result {rid} is unknown or has expired; run the query again")
        return head

    def rows(self, rid: str, offset: int = 0, limit: Optional[int] = None) -> Iterator[Any]:
        """Rows ``offset`` … ``offset + limit`` (all the rest when no limit)."""
        self.head(rid)
        try:
            f = open(self._path(rid), "rb")
        except FileNotFoundError:
            raise ResultGone(f"result {rid} is unknown or has expired") from None
        with f:
            f.readline()
            for i, line in enumerate(f):
                if i < offset:
                    continue
                if limit is not None and i >= offset + limit:
                    break
                yield orjson.loads(line)

    def page(self, rid: str, offset: int = 0, limit: int = 200) -> Dict[str, Any]:
        """One page for an agent: plain-value rows (or triples) and where to go next."""
        head = self.head(rid)
        offset, limit = max(int(offset), 0), min(max(int(limit), 1), 1000)
        raw = list(self.rows(rid, offset, limit))
        total = head["rows"]
        if head["kind"] == "select":
            rows = [r[0] for r in raw]
            doc: Dict[str, Any] = {"columns": head["vars"], "rows": rows}
        else:
            doc = {"triples": raw}
        doc.update(id=rid, dataset=head.get("dataset"), kind=head["kind"], offset=offset,
                   returned=len(raw), total_rows=total)
        end = offset + len(raw)
        doc["next_offset"] = end if end < total else None
        return doc


# --------------------------------------------------------------------------- #
# Splitting an answer
# --------------------------------------------------------------------------- #

def summary_and_rows(doc: Dict[str, Any]) -> Tuple[Dict[str, Any], List[Any]]:
    """A ``run_query`` document into the stored summary and its rows (SELECT:
    ``[plain row, W3C binding]`` pairs; CONSTRUCT: triples)."""
    meta = {k: doc.get(k) for k in ("dataset", "kind", "elapsed_seconds", "stats", "warnings")}
    meta["row_cap_hit"] = bool(doc.get("truncated"))
    if doc.get("kind") == "construct":
        return meta, list(doc.get("triples") or [])
    meta["vars"] = list((doc.get("head") or {}).get("vars") or [])
    table = doc.get("table") or []
    bindings = (doc.get("results") or {}).get("bindings") or []
    return meta, [[t, b] for t, b in zip(table, bindings)]


def doc_rows(doc: Dict[str, Any]) -> int:
    if doc.get("kind") == "construct":
        return len(doc.get("triples") or [])
    return len(doc.get("table") or doc.get("results", {}).get("bindings") or [])


def is_big(doc: Dict[str, Any], max_rows: int, max_bytes: int) -> bool:
    """Whether an answer is too big to hand back inline."""
    if doc.get("kind") == "ask":
        return False
    n = doc_rows(doc)
    if n > max_rows:
        return True
    return len(orjson.dumps(doc)) > max_bytes


# --------------------------------------------------------------------------- #
# Download formats
# --------------------------------------------------------------------------- #

def _nt_term(b: Dict[str, Any]) -> str:
    kind, value = b["type"], b["value"]
    if kind == "uri":
        return f"<{value}>"
    if kind == "bnode":
        return f"_:{value}"
    esc = (value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
           .replace("\r", "\\r").replace("\t", "\\t"))
    if "xml:lang" in b:
        return f'"{esc}"@{b["xml:lang"]}'
    if "datatype" in b:
        return f'"{esc}"^^<{b["datatype"]}>'
    return f'"{esc}"'


def _line(cells: List[str], dialect: str) -> bytes:
    buf = io.StringIO()
    csv.writer(buf, dialect=dialect, lineterminator="\r\n" if dialect == "excel" else "\n").writerow(cells)
    return buf.getvalue().encode()


def render(store: ResultStore, rid: str, fmt: str) -> Iterator[bytes]:
    """The stored answer in one wire format, streamed."""
    head = store.head(rid)
    select = head["kind"] == "select"
    if fmt == "nt":
        if select:
            raise ValueError("format=nt is for CONSTRUCT / DESCRIBE answers")
        for s, p, o in store.rows(rid):
            yield f"{s} {p} {o} .\n".encode()
        return
    if fmt in ("csv", "tsv"):
        if not select:
            raise ValueError(f"format={fmt} is for SELECT answers (use nt for CONSTRUCT)")
        variables = head["vars"]
        if fmt == "csv":
            yield _line(variables, "excel")
            for _, b in store.rows(rid):
                yield _line([b[v]["value"] if v in b else "" for v in variables], "excel")
        else:  # the SPARQL TSV form: N-Triples terms, tab separated
            yield ("\t".join("?" + v for v in variables) + "\n").encode()
            for _, b in store.rows(rid):
                yield ("\t".join(_nt_term(b[v]) if v in b else "" for v in variables) + "\n").encode()
        return
    if fmt == "srj" and not select:
        raise ValueError("format=srj is for SELECT answers")
    if select:
        yield b'{"head":{"vars":' + orjson.dumps(head["vars"]) + b'},"results":{"bindings":['
        first = True
        for _, b in store.rows(rid):
            yield (b"" if first else b",") + orjson.dumps(b)
            first = False
        yield b"]}"
        if fmt == "json":
            extra = {k: head.get(k) for k in ("dataset", "kind", "elapsed_seconds", "stats", "warnings")}
            extra["total_rows"] = head["rows"]
            yield b"," + orjson.dumps(extra)[1:-1]
        yield b"}"
    else:
        extra = {k: head.get(k) for k in ("dataset", "kind", "elapsed_seconds", "stats", "warnings")}
        extra["total_rows"] = head["rows"]
        yield orjson.dumps(extra)[:-1] + b',"triples":['
        first = True
        for t in store.rows(rid):
            yield (b"" if first else b",") + orjson.dumps(t)
            first = False
        yield b"]}"


_MEDIA = {"json": "application/json", "srj": "application/sparql-results+json",
          "csv": "text/csv; charset=utf-8", "tsv": "text/tab-separated-values; charset=utf-8",
          "nt": "application/n-triples; charset=utf-8"}

router = APIRouter(prefix="/api", tags=["results"])


@router.get("/results/{rid}")
def download_result(rid: str, format: str = "json"):
    """A stored query answer (from a sparql_query that was too big to return
    inline) in full: json, srj (W3C SPARQL results), csv, tsv or, for
    CONSTRUCT, nt. Needs the same token as every other route."""
    if format not in FORMATS:
        raise HTTPException(400, f"unknown format {format!r} (expected one of {', '.join(FORMATS)})")
    store = get_store()
    try:
        head = store.head(rid)
        if format == "nt" and head["kind"] == "select":
            raise ValueError("format=nt is for CONSTRUCT / DESCRIBE answers")
        if format in ("csv", "tsv", "srj") and head["kind"] != "select":
            raise ValueError(f"format={format} is for SELECT answers (use nt or json)")
    except ResultGone as e:
        raise HTTPException(404, str(e)) from None
    except ValueError as e:
        raise HTTPException(400, str(e)) from None
    ext = {"json": "json", "srj": "srj", "csv": "csv", "tsv": "tsv", "nt": "nt"}[format]
    return StreamingResponse(render(store, rid, format), media_type=_MEDIA[format],
                             headers={"Content-Disposition": f'attachment; filename="result-{rid[:8]}.{ext}"',
                                      "Cache-Control": "no-store"})


# --------------------------------------------------------------------------- #
# The shared store
# --------------------------------------------------------------------------- #

_store: Optional[ResultStore] = None
_store_key: Optional[tuple] = None


def get_store() -> ResultStore:
    """The store for the running server's settings (rebuilt if they change,
    which only a test does)."""
    global _store, _store_key
    s = state.settings
    if s is None:
        from .settings import Settings
        s = Settings.from_env()
    key = (str(s.results_dir), s.results_max_mb, s.results_ttl_s)
    if _store is None or key != _store_key:
        _store = ResultStore(s.results_dir, s.results_max_mb << 20, s.results_ttl_s)
        _store_key = key
    return _store
