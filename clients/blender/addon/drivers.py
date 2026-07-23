"""Live graph values inside Blender drivers.

Registers ``rete()`` in the driver namespace, so any animatable property can be
driven straight from a SPARQL query::

    # in a driver expression
    rete("SELECT (COUNT(*) AS ?n) WHERE { ?s a <https://w3id.org/rete/anatomy#Muscle> }")

Drivers are evaluated constantly — on every frame change, every redraw — so
results are memoised and a query that fails is remembered as failed. Without
that, one driver would issue thousands of range requests while you scrub.
"""

from __future__ import annotations

from typing import Any, Dict, Optional, Tuple

import bpy

from . import engine

#: (source, query, variable) -> value. Cleared explicitly, never on a timer:
#: a driver must be deterministic within a session or the viewport flickers.
_cache: Dict[Tuple[str, str, str], float] = {}

#: Set by the add-on preferences/scene settings so `rete()` can omit the source.
_default_source = ""


def set_default_source(source: str) -> None:
    global _default_source
    _default_source = source or ""


def clear_cache() -> int:
    count = len(_cache)
    _cache.clear()
    return count


def rete_value(query: str, source: str = "", variable: str = "", default: float = 0.0) -> float:
    """The first numeric value a query returns — the driver entry point.

    Returns ``default`` for anything that does not resolve to a number, because
    raising inside a driver disables it and Blender does not tell you why.
    """
    src = source or _default_source
    key = (src, query, variable)
    if key in _cache:
        return _cache[key]

    value = default
    try:
        result = engine.select(src, query)
        if result.rows:
            row = result.rows[0]
            var = variable or (result.vars[0] if result.vars else "")
            cell = row.get(var) or next(iter(row.values()), None)
            if cell is not None:
                number = cell.as_number()
                if number is not None:
                    value = number
    except Exception:
        value = default
    _cache[key] = value
    return value


def rete_count(pattern: str, source: str = "") -> float:
    """Convenience: how many solutions a graph pattern has.

    ``rete_count("?s a <...Muscle>")`` rather than spelling out the whole query.
    """
    return rete_value("SELECT (COUNT(*) AS ?n) WHERE { %s }" % pattern, source, "n")


def register() -> None:
    bpy.app.driver_namespace["rete"] = rete_value
    bpy.app.driver_namespace["rete_count"] = rete_count


def unregister() -> None:
    for name in ("rete", "rete_count"):
        bpy.app.driver_namespace.pop(name, None)
    _cache.clear()
