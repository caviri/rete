"""The running server's objects, set once by :func:`rete_server.app.create_app`.

Routes and MCP tools read them from here instead of from import-time globals,
so a test can build a server with any settings it likes.
"""
from __future__ import annotations

from typing import TYPE_CHECKING, Any, Dict, Optional

if TYPE_CHECKING:
    from .catalog import Catalog
    from .engine import Service
    from .settings import Settings

settings: "Optional[Settings]" = None
catalog: "Optional[Catalog]" = None
service: "Optional[Service]" = None


def error_status(exc: BaseException) -> int:
    """The HTTP status a service error maps to."""
    from .engine import Busy, QueryTimeout, RequestError, UnknownDataset

    if isinstance(exc, UnknownDataset):
        return 404
    if isinstance(exc, RequestError):
        return 400
    if isinstance(exc, Busy):
        return 503
    if isinstance(exc, (QueryTimeout, TimeoutError)):
        return 504
    return 502


def describe_error(exc: BaseException) -> str:
    from .engine import Busy, QueryTimeout, RequestError

    if isinstance(exc, (RequestError, Busy, QueryTimeout)):
        return str(exc)
    return f"{type(exc).__name__}: {exc}"


def public_entry(entry: Dict[str, Any]) -> Dict[str, Any]:
    """A catalog entry without server-side paths."""
    return {k: v for k, v in entry.items() if k != "local_path"}
