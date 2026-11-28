"""Long queries and large results, for the MCP tools.

* **Large results.** :func:`shape_sparql` keeps a big answer in the result
  store (see ``results.py``) and returns a summary, a handle and a
  ``resource_link``; ``result_rows`` and ``rete://results/<id>`` read it back
  in pages.
* **Tasks.** ``sparql_query``, ``validate_shacl`` and ``build_rete`` are
  task-optional (the ``io.modelcontextprotocol/tasks`` extension): a client
  that declared the extension gets a task to poll, any other client gets the
  ordinary synchronous answer. :func:`run_blocking` runs the engine call off
  the event loop and, inside a task, keeps the task's status message current.

The engine cannot be interrupted, so cancelling a task discards its result
but the query it started runs to its end.
"""
from __future__ import annotations

import asyncio
import functools
import inspect
import time
from datetime import timedelta
from typing import Any, Callable, Dict, List, Optional

import anyio
import orjson
from fastmcp import FastMCP
from fastmcp.exceptions import ToolError
from fastmcp.tools import ToolResult
from fastmcp.utilities.tasks import TaskConfig
from mcp_types import ResourceLink, TextContent

from . import results as results_mod
from . import state

PREVIEW_ROWS = 20
TASK_POLL = timedelta(seconds=2)
# Task-optional: a task for a client that declared the extension, a plain answer for any other.
TASK_CONFIG = TaskConfig(mode="optional", poll_interval=TASK_POLL)


def tool_errors(fn):
    """Service errors become tool errors with a clean message (sync or async tools)."""
    if inspect.iscoroutinefunction(fn):
        @functools.wraps(fn)
        async def awrapper(*args, **kwargs):
            try:
                return await fn(*args, **kwargs)
            except ToolError:
                raise
            except Exception as e:
                raise ToolError(state.describe_error(e)) from None
        return awrapper

    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        try:
            return fn(*args, **kwargs)
        except ToolError:
            raise
        except Exception as e:
            raise ToolError(state.describe_error(e)) from None
    return wrapper


def in_task() -> bool:
    """True inside a background task's worker (False in an ordinary call)."""
    try:
        from fastmcp_tasks.context import get_task_context
    except ImportError:  # pragma: no cover - the dependency is declared
        return False
    return get_task_context() is not None


def lane() -> str:
    """The engine lane for this call: tasks use their own handle and time limit."""
    return "long" if in_task() else "quick"


async def run_blocking(fn: Callable[..., Any], *args: Any, progress: Any = None,
                       label: str = "", **kwargs: Any) -> Any:
    """Run a blocking engine call in a thread. In a task, set the task's
    status message every couple of seconds (``label``, seconds elapsed), so a
    polling client can show something better than "working".

    A cancelled task stops waiting at once and drops the result; the thread
    (the engine) still finishes the query it is in.
    """
    ticker: Optional[asyncio.Task] = None
    started = time.monotonic()
    if progress is not None and in_task():
        async def tick() -> None:
            while True:
                try:
                    await progress.set_message(f"{label} ({int(time.monotonic() - started)} s elapsed)")
                except Exception:
                    return
                await asyncio.sleep(2)
        ticker = asyncio.create_task(tick())
    try:
        return await anyio.to_thread.run_sync(functools.partial(fn, *args, **kwargs),
                                              abandon_on_cancel=True)
    finally:
        if ticker is not None:
            ticker.cancel()


# --------------------------------------------------------------------------- #
# Big answers
# --------------------------------------------------------------------------- #

def _base_url() -> str:
    """Where the download links point: the public URL, else the request's own
    Host (behind a TLS proxy set RETE_PUBLIC_URL, or the proxy's
    X-Forwarded-Proto), else nothing (relative links). A task has no request."""
    s = state.settings
    if s is not None and s.public_url:
        return s.public_url
    try:
        from fastmcp.server.dependencies import get_http_headers

        headers = get_http_headers(include_all=True)
    except Exception:
        return ""
    host = headers.get("x-forwarded-host") or headers.get("host")
    if not host:
        return ""
    proto = (headers.get("x-forwarded-proto") or "http").split(",")[0].strip()
    return f"{proto}://{host}"


def shape_sparql(doc: Dict[str, Any]) -> Any:
    """An answer as the MCP tool returns it: unchanged when it is small; when
    it is big, stored, and a summary with a handle comes back instead."""
    s = state.settings
    if s is None or not results_mod.is_big(doc, s.inline_rows, s.inline_bytes):
        return doc
    store = results_mod.get_store()
    meta, rows = results_mod.summary_and_rows(doc)
    handle = store.put(meta, rows)
    base = _base_url()
    handle["download"] = {k: base + v for k, v in handle["download"].items()}
    total = len(rows)
    out: Dict[str, Any] = {k: doc.get(k) for k in ("dataset", "kind", "elapsed_seconds", "stats", "warnings")}
    out["truncated"] = True
    out["total_rows"] = total
    if doc.get("kind") == "construct":
        out["triples"] = [t for t in rows[:PREVIEW_ROWS]]
    else:
        out["head"] = doc.get("head")
        out["table"] = [r[0] for r in rows[:PREVIEW_ROWS]]
    out["shown_rows"] = min(PREVIEW_ROWS, total)
    if doc.get("truncated"):
        out["row_cap_hit"] = True
    out["result"] = handle
    out["note"] = (f"{total} rows; only the first {out['shown_rows']} are shown. Page through the rest "
                   f"with result_rows(result='{handle['id']}', offset=…), or save it to disk with "
                   f"`curl -H 'Authorization: Bearer …' '{handle['download'].get('csv') or handle['download'].get('nt')}'`. "
                   "To keep it small, aggregate (COUNT, GROUP BY) or add a LIMIT.")
    if doc.get("truncated"):
        out["note"] += " The row cap was reached, so the stored answer is itself cut."
    text = orjson.dumps(out).decode()
    link = ResourceLink(name=f"result-{handle['id'][:8]}", uri=handle["uri"],
                        description=f"Full answer, {total} rows (expires {handle['expires']})",
                        mimeType="application/json")
    return ToolResult(content=[TextContent(type="text", text=text), link], structured_content=out)


# --------------------------------------------------------------------------- #
# Reading a stored answer back
# --------------------------------------------------------------------------- #

def read_page(ref: str, offset: int = 0, limit: int = 200) -> Dict[str, Any]:
    store = results_mod.get_store()
    try:
        return store.page(store.parse_ref(ref), offset, limit)
    except results_mod.ResultGone as e:
        raise ToolError(str(e)) from None


def register_results(mcp: FastMCP, read_only: Dict[str, Any]) -> None:
    mcp.tool(tool_errors(result_rows), annotations=read_only, task=False)

    @mcp.resource("rete://results/{id}{?offset,limit}", mime_type="application/json",
                  name="stored_result", title="A stored query answer, one page")
    def stored_result(id: str, offset: int = 0, limit: int = 200) -> str:
        """One page of a stored sparql_query answer (default 200 rows from
        `offset`); `next_offset` says where the next page starts."""
        return orjson.dumps(read_page(id, offset, limit)).decode()


def result_rows(result: str, offset: int = 0, limit: int = 200) -> Dict[str, Any]:
    """One page of a stored sparql_query answer. `result` is the handle's `id`
    (or its `rete://results/<id>` uri) from a sparql_query that came back
    `truncated`. Returns `rows` (or `triples`), `total_rows` and `next_offset`
    (null at the end). Up to 1000 rows a page; results expire after an hour."""
    return read_page(result, offset, limit)
