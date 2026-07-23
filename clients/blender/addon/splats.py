"""Gaussian splats (3DGS) from a graph.

A splat URL — a 3D Gaussian Splatting reconstruction, the photoreal output of a
scan — becomes scene content the same way a ``.glb`` does. Splats have no native
Blender renderer, so this follows the same handoff pattern as IFC: if a 3DGS
add-on (e.g. KIRI Engine's *3DGS Render*) is installed, let **it** import and
render the splat properly; otherwise fall back to an honest **point-cloud
preview** — the Gaussian centres and their base colours, parsed here — so you at
least see the shape and where it sits, with a clear message about installing the
add-on for real rendering.

Two formats are parsed for the preview: the canonical 3DGS ``.ply`` (the direct
training output) and antimatter15's compact ``.splat``. ``.ksplat``/``.spz`` are
web/compressed variants; with an add-on that reads them they still work, but the
preview asks the user to convert to ``.ply`` first.

Placement note: a splat's stored attributes (position, anisotropic scale,
quaternion rotation, spherical-harmonic colour, opacity) go inconsistent if you
bake an ordinary Blender transform onto the object. The builder therefore never
transforms a splat object directly — it parents it to an empty and moves the
empty. Nothing here mutates a splat's own matrix.
"""

from __future__ import annotations

import math
import os
import re
import struct
from typing import List, Optional, Tuple

import bpy

#: How many Gaussian centres the fallback preview shows (evenly sampled from the
#: full set). Millions of points would choke the viewport for no extra insight.
PREVIEW_POINTS = 200_000

SPLAT_HINT = (
    "Gaussian-splat rendering needs a 3DGS add-on (e.g. KIRI Engine's "
    "'3DGS Render'). Install it to import and render splats properly. "
    "For .ksplat/.spz, convert to a 3DGS .ply first."
)

# Spherical-harmonics DC term: colour = 0.5 + C0 * f_dc.
_SH_C0 = 0.28209479177387814


def is_splat_ply(path: str) -> bool:
    """True if a ``.ply`` is a Gaussian splat, from its header properties."""
    try:
        with open(path, "rb") as fh:
            head = fh.read(4096)
    except OSError:
        return False
    if not head.startswith(b"ply"):
        return False
    text = head.split(b"end_header", 1)[0].decode("latin-1", "replace")
    # 3DGS PLYs carry SH DC colour (f_dc_*) and per-Gaussian scale/rotation.
    return ("f_dc_0" in text) or ("scale_0" in text and "rot_0" in text)


# ----------------------------------------------------------- add-on handoff

# Operator-id evidence for a splat importer. Add-on operator ids drift, so we
# discover rather than hard-code — but carefully: a mere "gaussian" or "splat"
# substring collides with built-ins (e.g. graph.gaussian_smooth), so a candidate
# must ALSO read like an import, or carry a strong compound splat token.
_FAMILY_HINTS = ("3dgs", "gaussian", "gsplat", "splat", "kiri")
_IMPORT_HINTS = ("import", "load", "open", "readfile")
_STRONG_HINTS = ("3dgs", "gsplat", "ksplat", "gaussiansplat")


def find_splat_importer():
    """A registered 3DGS import operator, or ``None``.

    Scans ``bpy.ops`` for an operator whose id looks like a splat *importer*. A
    splat-family substring alone is not enough — it must also read like an import
    or carry a strong compound token — so ``graph.gaussian_smooth`` and similar
    built-ins are not mistaken for one.
    """
    best = None
    best_score = -1
    for group_name in dir(bpy.ops):
        group = getattr(bpy.ops, group_name, None)
        if group is None:
            continue
        try:
            names = dir(group)
        except Exception:
            continue
        for op_name in names:
            norm = re.sub(r"[^a-z0-9]", "", f"{group_name}.{op_name}".lower())
            if not any(h in norm for h in _FAMILY_HINTS):
                continue
            is_import = any(h in norm for h in _IMPORT_HINTS)
            is_strong = any(h in norm for h in _STRONG_HINTS)
            if not (is_import or is_strong):
                continue
            score = (2 if is_strong else 0) + (1 if is_import else 0)
            if score > best_score:
                best, best_score = getattr(group, op_name), score
    return best


def _call_importer(op, path: str) -> bool:
    """Call a discovered importer with whatever keyword it expects."""
    for kwargs in ({"filepath": path}, {"filename": path}, {"path": path}, {}):
        try:
            result = op(**kwargs)
        except TypeError:
            continue
        except RuntimeError:
            return False
        if isinstance(result, set) and "FINISHED" in result:
            return True
    return False


def _import_via_addon(path: str) -> Optional[List["bpy.types.Object"]]:
    op = find_splat_importer()
    if op is None:
        return None
    before = set(bpy.data.objects)
    if not _call_importer(op, path):
        return None
    new = [o for o in bpy.data.objects if o not in before]
    for obj in new:
        obj["rete:splat"] = True
    return new or None


# ------------------------------------------------------------ preview parse


def _read_ply_header(fh) -> Tuple[int, List[Tuple[str, str]], str]:
    """Parse a binary PLY header; return ``(vertex_count, properties, format)``.

    ``properties`` is ``[(name, type)]`` for the vertex element, in order.
    """
    magic = fh.readline().strip()
    if magic != b"ply":
        raise IOError("not a PLY file")
    fmt = "binary_little_endian"
    count = 0
    props: List[Tuple[str, str]] = []
    in_vertex = False
    while True:
        line = fh.readline()
        if not line:
            raise IOError("truncated PLY header")
        parts = line.split()
        if not parts:
            continue
        key = parts[0]
        if key == b"format":
            fmt = parts[1].decode("ascii", "replace")
        elif key == b"element":
            in_vertex = parts[1] == b"vertex"
            if in_vertex:
                count = int(parts[2])
        elif key == b"property" and in_vertex:
            props.append((parts[2].decode("ascii", "replace"), parts[1].decode("ascii", "replace")))
        elif key == b"end_header":
            break
    return count, props, fmt


_PLY_STRUCT = {
    "char": ("b", 1), "int8": ("b", 1), "uchar": ("B", 1), "uint8": ("B", 1),
    "short": ("h", 2), "int16": ("h", 2), "ushort": ("H", 2), "uint16": ("H", 2),
    "int": ("i", 4), "int32": ("i", 4), "uint": ("I", 4), "uint32": ("I", 4),
    "float": ("f", 4), "float32": ("f", 4), "double": ("d", 8), "float64": ("d", 8),
}


def _sigmoid(x: float) -> float:
    if x >= 0:
        return 1.0 / (1.0 + math.exp(-x))
    e = math.exp(x)
    return e / (1.0 + e)


def parse_ply_splat(path: str, limit: int = PREVIEW_POINTS):
    """3DGS ``.ply`` -> ``(positions, colors)`` for a preview, evenly sampled."""
    with open(path, "rb") as fh:
        count, props, fmt = _read_ply_header(fh)
        if "ascii" in fmt:
            raise IOError("ASCII PLY splats are not supported for preview")
        endian = "<" if "little" in fmt else ">"

        offsets = {}
        stride = 0
        codes = []
        for name, ptype in props:
            code, size = _PLY_STRUCT.get(ptype, ("f", 4))
            offsets[name] = (stride, code, size)
            codes.append((code, size))
            stride += size
        if not all(k in offsets for k in ("x", "y", "z")):
            raise IOError("PLY splat has no position")

        step = max(1, count // limit)
        positions: List[Tuple[float, float, float]] = []
        colors: List[Tuple[float, float, float, float]] = []
        record = struct.Struct(endian + "".join(c for c, _ in codes))
        buf = fh.read(stride * count)

    def field(rec: bytes, name: str, default: float = 0.0) -> float:
        spec = offsets.get(name)
        if spec is None:
            return default
        off, code, size = spec
        return struct.unpack_from(endian + code, rec, off)[0]

    for i in range(0, count, step):
        rec = buf[i * stride:(i + 1) * stride]
        if len(rec) < stride:
            break
        positions.append((field(rec, "x"), field(rec, "y"), field(rec, "z")))
        r = 0.5 + _SH_C0 * field(rec, "f_dc_0")
        g = 0.5 + _SH_C0 * field(rec, "f_dc_1")
        b = 0.5 + _SH_C0 * field(rec, "f_dc_2")
        a = _sigmoid(field(rec, "opacity", 4.0))
        colors.append((_clamp(r), _clamp(g), _clamp(b), _clamp(a)))
    return positions, colors


def parse_dot_splat(path: str, limit: int = PREVIEW_POINTS):
    """antimatter15 ``.splat`` -> ``(positions, colors)``; 32 bytes per splat."""
    with open(path, "rb") as fh:
        data = fh.read()
    count = len(data) // 32
    step = max(1, count // limit)
    positions: List[Tuple[float, float, float]] = []
    colors: List[Tuple[float, float, float, float]] = []
    for i in range(0, count, step):
        base = i * 32
        x, y, z = struct.unpack_from("<fff", data, base)
        r, g, b, a = data[base + 24], data[base + 25], data[base + 26], data[base + 27]
        positions.append((x, y, z))
        colors.append((r / 255.0, g / 255.0, b / 255.0, a / 255.0))
    return positions, colors


def _clamp(v: float) -> float:
    return 0.0 if v < 0.0 else (1.0 if v > 1.0 else v)


def _preview_object(name: str, positions, colors) -> Optional["bpy.types.Object"]:
    if not positions:
        return None
    # Centre the preview on its own centroid so it lands at the placement point
    # when parented to the builder's empty. This is our own plain mesh, so moving
    # its vertices is safe — the desync concern is only for real splat objects.
    n = len(positions)
    cx = sum(p[0] for p in positions) / n
    cy = sum(p[1] for p in positions) / n
    cz = sum(p[2] for p in positions) / n
    positions = [(p[0] - cx, p[1] - cy, p[2] - cz) for p in positions]
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(list(positions), [], [])
    mesh.update()
    attr = mesh.attributes.new(name="splat_color", type="FLOAT_COLOR", domain="POINT")
    flat: List[float] = []
    for c in colors:
        flat.extend(c)
    attr.data.foreach_set("color", flat)
    obj = bpy.data.objects.new(name, mesh)
    obj["rete:splat"] = True
    obj["rete:splatPreview"] = True
    return obj


def import_splat(path: str, url: str, *, limit: int = PREVIEW_POINTS) -> Tuple[List["bpy.types.Object"], str, bool]:
    """Import a splat: via a 3DGS add-on if present, else a point-cloud preview.

    Returns ``(objects, note, via_addon)``. Add-on objects come already linked and
    set up by the add-on; preview objects are returned unlinked for the caller to
    place. Raises ``IOError`` only when nothing can be shown at all (an
    unparseable ``.ksplat`` with no add-on).
    """
    via_addon = _import_via_addon(path)
    if via_addon:
        return via_addon, "", True

    ext = os.path.splitext(path)[1].lower()
    label = f"{os.path.basename(url).rsplit('.', 1)[0]} (splat preview)"
    try:
        if ext == ".ply":
            positions, colors = parse_ply_splat(path, limit=limit)
        elif ext == ".splat":
            positions, colors = parse_dot_splat(path, limit=limit)
        else:  # .ksplat / .spz — compressed web formats we do not parse
            raise IOError(
                f"{os.path.basename(url)}: no 3DGS add-on installed, and "
                f".ksplat/.spz cannot be previewed. {SPLAT_HINT}"
            )
    except IOError:
        raise
    except Exception as exc:
        raise IOError(f"could not read splat {os.path.basename(url)}: {exc}") from exc

    obj = _preview_object(label, positions, colors)
    if obj is None:
        raise IOError(f"no Gaussians read from {os.path.basename(url)}")
    return [obj], f"showing {len(positions):,}-point preview — {SPLAT_HINT}", False
