"""Stamping RDF statements onto Blender objects as custom properties.

This is the "inherit the properties" half of the add-on. Every statement about
an entity becomes a custom property on its object, which means:

* it shows up in Object Properties ▸ Custom Properties, with the full predicate
  IRI as the tooltip;
* numeric values are **drivable** — ``["mass"]`` can drive a scale, a shader
  input, a modifier, anything Blender can drive;
* Geometry Nodes can read them through the object's custom properties;
* and the round trip back to RDF is lossless, because the local-name to
  predicate-IRI mapping travels with the object.

Keys are the predicate's local name, sanitised to what Blender driver paths
accept. When two different predicates share a local name, the second one gets a
short suffix rather than silently overwriting the first.
"""

from __future__ import annotations

import json
import re
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

import bpy

#: Reserved keys carrying the object's graph identity.
IRI = "rete:iri"
SOURCE = "rete:source"
QUERY = "rete:query"
PREDICATES = "rete:predicates"
MULTI = "rete:multi"
CLASSES = "rete:classes"
DATATYPES = "rete:datatypes"

RESERVED = {IRI, SOURCE, QUERY, PREDICATES, MULTI, CLASSES, DATATYPES}

_SANITIZE_RE = re.compile(r"[^A-Za-z0-9_]")
_LOCAL_RE = re.compile(r"[^#/:]+$")

_XSD = "http://www.w3.org/2001/XMLSchema#"
_BOOL = _XSD + "boolean"


def local_name(iri: str) -> str:
    """The readable tail of an IRI: the bit after the last ``#``, ``/`` or ``:``."""
    match = _LOCAL_RE.search(iri or "")
    return match.group(0) if match else (iri or "")


def prop_key(predicate: str) -> str:
    """A Blender-safe custom-property key for a predicate IRI."""
    name = _SANITIZE_RE.sub("_", local_name(predicate)).strip("_")
    if not name:
        name = "prop"
    if name[0].isdigit():
        name = "p_" + name
    return name


def _coerce(cell) -> Any:
    """The most useful Python value for a cell.

    Numbers become floats so they can drive things; booleans stay booleans;
    everything else stays a string, because Blender custom properties cannot
    hold a typed RDF term and the datatype is recorded separately anyway.
    """
    if cell.kind != "literal":
        return cell.value
    if cell.datatype == _BOOL:
        return cell.value.strip().lower() in ("true", "1")
    number = cell.as_number()
    return cell.value if number is None else number


def stamp_identity(obj: "bpy.types.Object", iri: str, source: str, query: str = "") -> None:
    """Record which graph node an object *is*, and where it came from."""
    obj[IRI] = iri
    obj[SOURCE] = source
    if query:
        obj[QUERY] = query
    _describe(obj, IRI, "The IRI of the graph node this object represents")
    _describe(obj, SOURCE, "The .rete file this object was imported from")


def _describe(obj: "bpy.types.Object", key: str, text: str) -> None:
    """Attach a tooltip to a custom property, where the API allows it."""
    try:
        ui = obj.id_properties_ui(key)
    except (TypeError, KeyError, AttributeError):
        return
    try:
        ui.update(description=text)
    except (TypeError, ValueError):
        pass


def stamp(
    obj: "bpy.types.Object",
    statements: Sequence[Tuple[str, Any]],
    *,
    skip: Iterable[str] = (),
    max_text: int = 400,
) -> int:
    """Write ``[(predicate_iri, cell)]`` onto an object. Returns keys written.

    Repeated predicates collect into a JSON array so nothing is lost, and the
    predicate map is updated so :mod:`.export` can rebuild the original IRIs.
    """
    skip_set = set(skip)
    by_key: Dict[str, List[Any]] = {}
    predicates: Dict[str, str] = dict(_load_map(obj, PREDICATES))
    datatypes: Dict[str, str] = dict(_load_map(obj, DATATYPES))
    classes: List[str] = list(_load_list(obj, CLASSES))

    for predicate, cell in statements:
        if predicate in skip_set:
            continue
        if predicate == "http://www.w3.org/1999/02/22-rdf-syntax-ns#type":
            if cell.value not in classes:
                classes.append(cell.value)
            continue

        key = prop_key(predicate)
        # A local-name collision between two predicates must not lose data.
        if key in predicates and predicates[key] != predicate:
            key = f"{key}_{abs(hash(predicate)) % 997:03d}"
        predicates[key] = predicate
        if getattr(cell, "datatype", ""):
            datatypes[key] = cell.datatype

        value = _coerce(cell)
        if isinstance(value, str) and len(value) > max_text:
            value = value[: max_text - 1] + "…"
        by_key.setdefault(key, []).append(value)

    multi: List[str] = []
    for key, values in by_key.items():
        if key in RESERVED:
            continue
        if len(values) == 1:
            obj[key] = values[0]
        else:
            # Blender ID properties cannot hold mixed or string arrays; JSON keeps
            # multi-valued predicates intact and readable in the UI.
            obj[key] = json.dumps(values, ensure_ascii=False)
            multi.append(key)
        _describe(obj, key, predicates.get(key, key))

    if predicates:
        obj[PREDICATES] = json.dumps(predicates, ensure_ascii=False, separators=(",", ":"))
    if datatypes:
        obj[DATATYPES] = json.dumps(datatypes, ensure_ascii=False, separators=(",", ":"))
    if multi:
        obj[MULTI] = json.dumps(sorted(set(multi + list(_load_list(obj, MULTI)))))
    if classes:
        obj[CLASSES] = json.dumps(classes, ensure_ascii=False)
    return len(by_key)


def stamp_row(obj: "bpy.types.Object", row: Dict[str, Any], *, skip: Iterable[str] = ()) -> int:
    """Write a query row's cells onto an object, keyed by variable name.

    Used when the deep property pass is off: the columns the user selected are
    still worth having, even without their predicate IRIs.
    """
    skip_set = set(skip)
    written = 0
    for var, cell in row.items():
        if var in skip_set or cell is None:
            continue
        key = _SANITIZE_RE.sub("_", var)
        if key in RESERVED or not key:
            continue
        obj[key] = _coerce(cell)
        written += 1
    return written


# --------------------------------------------------------------------- reading


def _load_map(obj: "bpy.types.Object", key: str) -> Dict[str, str]:
    raw = obj.get(key)
    if not raw:
        return {}
    try:
        value = json.loads(raw)
        return value if isinstance(value, dict) else {}
    except (ValueError, TypeError):
        return {}


def _load_list(obj: "bpy.types.Object", key: str) -> List[str]:
    raw = obj.get(key)
    if not raw:
        return []
    try:
        value = json.loads(raw)
        return value if isinstance(value, list) else []
    except (ValueError, TypeError):
        return []


def iri_of(obj: "bpy.types.Object") -> str:
    return str(obj.get(IRI, "")) if obj else ""


def source_of(obj: "bpy.types.Object") -> str:
    return str(obj.get(SOURCE, "")) if obj else ""


def predicate_map(obj: "bpy.types.Object") -> Dict[str, str]:
    return _load_map(obj, PREDICATES)


def datatype_map(obj: "bpy.types.Object") -> Dict[str, str]:
    return _load_map(obj, DATATYPES)


def classes_of(obj: "bpy.types.Object") -> List[str]:
    return _load_list(obj, CLASSES)


def multi_keys(obj: "bpy.types.Object") -> List[str]:
    return _load_list(obj, MULTI)


def user_keys(obj: "bpy.types.Object") -> List[str]:
    """Custom property keys that carry data, excluding the bookkeeping ones."""
    return [k for k in obj.keys() if k not in RESERVED and not k.startswith("_")]


def values_of(obj: "bpy.types.Object", key: str) -> List[Any]:
    """A property's values, unpacking the JSON array form of multi-valued ones."""
    raw = obj.get(key)
    if raw is None:
        return []
    if key in multi_keys(obj) and isinstance(raw, str):
        try:
            parsed = json.loads(raw)
            if isinstance(parsed, list):
                return parsed
        except ValueError:
            pass
    if hasattr(raw, "to_list"):
        return list(raw.to_list())
    return [raw]


def number_of(obj: "bpy.types.Object", key: str) -> Optional[float]:
    """A property as a float, or ``None`` when it is not numeric."""
    values = values_of(obj, key)
    if not values:
        return None
    try:
        return float(values[0])
    except (TypeError, ValueError):
        return None


def objects_with_iri(context=None) -> Dict[str, "bpy.types.Object"]:
    """Every object in the file that carries a graph identity, by IRI."""
    scene = (context or bpy.context).scene
    out: Dict[str, "bpy.types.Object"] = {}
    for obj in scene.objects:
        iri = obj.get(IRI)
        if iri:
            out[str(iri)] = obj
    return out


def set_range(obj: "bpy.types.Object", key: str, low: float, high: float) -> None:
    """Give a numeric property a UI slider range, so it is pleasant to drive."""
    try:
        ui = obj.id_properties_ui(key)
        ui.update(min=low, max=high, soft_min=low, soft_max=high)
    except (TypeError, KeyError, AttributeError, ValueError):
        pass
