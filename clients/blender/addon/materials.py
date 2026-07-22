"""Turning RDF values into Blender materials.

Four sources of colour, in the order the builder prefers them:

1. an explicit colour literal on the row,
2. an image URL, which becomes a textured material,
3. a numeric column, mapped through a perceptually uniform ramp,
4. the entity's class, which gets a stable colour derived from its IRI.

Materials are cached by key, so a thousand rows of the same class share one
material rather than creating a thousand near-identical ones.
"""

from __future__ import annotations

import colorsys
import hashlib
import re
from typing import Dict, Optional, Sequence, Tuple

import bpy

from . import assets

RGBA = Tuple[float, float, float, float]

#: Viridis, sampled at nine stops. Perceptually uniform, colour-blind safe, and
#: it reads correctly under Blender's default lighting.
VIRIDIS: Sequence[Tuple[float, float, float]] = (
    (0.267, 0.005, 0.329), (0.283, 0.141, 0.458), (0.254, 0.265, 0.530),
    (0.207, 0.372, 0.553), (0.164, 0.471, 0.558), (0.128, 0.567, 0.551),
    (0.135, 0.659, 0.518), (0.267, 0.749, 0.441), (0.478, 0.821, 0.318),
)

CSS_COLORS: Dict[str, Tuple[float, float, float]] = {
    "black": (0, 0, 0), "white": (1, 1, 1), "red": (1, 0, 0), "green": (0, 0.5, 0),
    "blue": (0, 0, 1), "yellow": (1, 1, 0), "cyan": (0, 1, 1), "magenta": (1, 0, 1),
    "grey": (0.5, 0.5, 0.5), "gray": (0.5, 0.5, 0.5), "orange": (1, 0.647, 0),
    "purple": (0.5, 0, 0.5), "brown": (0.647, 0.165, 0.165), "pink": (1, 0.753, 0.796),
    "silver": (0.753, 0.753, 0.753), "gold": (1, 0.843, 0), "navy": (0, 0, 0.5),
    "teal": (0, 0.5, 0.5), "olive": (0.5, 0.5, 0), "maroon": (0.5, 0, 0),
}

_RGB_RE = re.compile(
    r"rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.%]+))?\s*\)",
    re.IGNORECASE,
)


def srgb_to_linear(c: float) -> float:
    """Blender's shader inputs are linear; colour literals are sRGB."""
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def parse_color(value: str) -> Optional[RGBA]:
    """Parse ``#rgb`` / ``#rrggbb(aa)`` / ``rgb()`` / a CSS colour name."""
    if not value:
        return None
    v = value.strip().lower()

    if v.startswith("#"):
        h = v[1:]
        if len(h) in (3, 4):
            h = "".join(c * 2 for c in h)
        if len(h) not in (6, 8) or any(c not in "0123456789abcdef" for c in h):
            return None
        vals = [int(h[i : i + 2], 16) / 255.0 for i in range(0, len(h), 2)]
        alpha = vals[3] if len(vals) == 4 else 1.0
        return (*(srgb_to_linear(c) for c in vals[:3]), alpha)  # type: ignore[return-value]

    m = _RGB_RE.match(v)
    if m:
        parts = []
        for raw in m.groups()[:3]:
            n = float(raw)
            parts.append(n / 255.0 if n > 1.0 else n)
        alpha_raw = m.group(4)
        alpha = 1.0
        if alpha_raw:
            alpha = float(alpha_raw.rstrip("%")) / (100.0 if "%" in alpha_raw else 1.0)
        return (*(srgb_to_linear(c) for c in parts), alpha)  # type: ignore[return-value]

    if v in CSS_COLORS:
        return (*(srgb_to_linear(c) for c in CSS_COLORS[v]), 1.0)  # type: ignore[return-value]
    return None


def color_for_key(key: str, *, saturation: float = 0.55, value: float = 0.85) -> RGBA:
    """A stable, pleasant colour for an arbitrary string.

    The hue comes from a hash of the key, so a class keeps its colour across
    sessions and across separate imports of the same dataset.
    """
    digest = hashlib.sha256(key.encode("utf-8")).digest()
    hue = digest[0] / 255.0
    # Nudge away from the muddy yellow-green band where labels read poorly.
    if 0.12 < hue < 0.20:
        hue += 0.12
    r, g, b = colorsys.hsv_to_rgb(hue % 1.0, saturation, value)
    return (srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0)


def ramp(t: float) -> RGBA:
    """Sample the viridis ramp at ``t`` in [0, 1] (already linear)."""
    t = 0.0 if t != t else max(0.0, min(1.0, t))  # NaN -> 0
    pos = t * (len(VIRIDIS) - 1)
    i = int(pos)
    frac = pos - i
    a = VIRIDIS[i]
    b = VIRIDIS[min(i + 1, len(VIRIDIS) - 1)]
    return (
        a[0] + (b[0] - a[0]) * frac,
        a[1] + (b[1] - a[1]) * frac,
        a[2] + (b[2] - a[2]) * frac,
        1.0,
    )


# ------------------------------------------------------------------ materials


def _principled(mat: "bpy.types.Material"):
    for node in mat.node_tree.nodes:
        if node.type == "BSDF_PRINCIPLED":
            return node
    return None


def _set_input(node, names: Sequence[str], value) -> None:
    """Set the first input that exists — Principled's sockets were renamed
    between Blender 3.x and 4.x, and the add-on supports both."""
    for name in names:
        socket = node.inputs.get(name)
        if socket is not None:
            socket.default_value = value
            return


def solid(key: str, color: RGBA, *, roughness: float = 0.45) -> "bpy.types.Material":
    """A cached single-colour material."""
    name = f"rete:{key}"
    mat = bpy.data.materials.get(name)
    if mat is not None:
        return mat
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = _principled(mat)
    if bsdf is not None:
        _set_input(bsdf, ("Base Color",), color)
        _set_input(bsdf, ("Roughness",), roughness)
        if color[3] < 1.0:
            _set_input(bsdf, ("Alpha",), color[3])
            mat.blend_method = "BLEND"
    # Also drives Solid viewport shading, where most modelling actually happens.
    mat.diffuse_color = color
    return mat


def textured(url: str, *, max_pixels: int = 2048) -> Optional["bpy.types.Material"]:
    """A material whose base colour is the image at ``url``."""
    image = assets.load_image(url, max_pixels=max_pixels)
    if image is None:
        return None
    name = f"rete:tex:{image.name}"
    mat = bpy.data.materials.get(name)
    if mat is not None:
        return mat
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    tree = mat.node_tree
    bsdf = _principled(mat)
    tex = tree.nodes.new("ShaderNodeTexImage")
    tex.image = image
    tex.location = (-320, 260)
    if bsdf is not None:
        tree.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
        _set_input(bsdf, ("Roughness",), 0.6)
    return mat


def emissive(key: str, color: RGBA, strength: float = 2.0) -> "bpy.types.Material":
    """A glowing material — used for highlights and for time cursors."""
    name = f"rete:glow:{key}"
    mat = bpy.data.materials.get(name)
    if mat is not None:
        return mat
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = _principled(mat)
    if bsdf is not None:
        _set_input(bsdf, ("Base Color",), color)
        _set_input(bsdf, ("Emission Color", "Emission"), color)
        _set_input(bsdf, ("Emission Strength",), strength)
    mat.diffuse_color = color
    return mat


def assign(obj: "bpy.types.Object", mat: "bpy.types.Material") -> None:
    """Put a material on an object, replacing whatever it had.

    Materials are assigned to the *object*, not the mesh, so linked copies
    sharing one mesh can still be coloured independently — which is exactly the
    case when one system ``.glb`` supplies hundreds of rows.
    """
    if obj.type not in {"MESH", "CURVE", "SURFACE", "META", "FONT", "VOLUME"}:
        return
    if not obj.material_slots:
        obj.data.materials.append(None)
    slot = obj.material_slots[0]
    slot.link = "OBJECT"
    slot.material = mat
