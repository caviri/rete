"""The way back: a Blender scene serialised as RDF, and built into a ``.rete``.

Import is only half a client. Once a scene carries graph identity, Blender
becomes an *authoring* tool for 3D knowledge graphs: model or arrange something,
annotate it with custom properties, and export a real queryable file.

Objects that came from a graph keep their IRIs and their original predicates, so
a round trip preserves what it read and adds what you changed. Objects you
modelled yourself get minted IRIs under a base you choose.

The emitted scene vocabulary lives at ``https://w3id.org/rete/scene#`` and is
described in the file itself, so the export is self-explaining.
"""

from __future__ import annotations

import json
import math
import re
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

import bpy

from . import engine
from . import props as rprops

SCENE = "https://w3id.org/rete/scene#"
GEO3 = "https://w3id.org/rete/geo3#"
GEO = "http://www.opengis.net/ont/geosparql#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
XSD = "http://www.w3.org/2001/XMLSchema#"

WKT3D_DT = GEO3 + "wktLiteral"

_IRI_SAFE_RE = re.compile(r"[^A-Za-z0-9_.~-]+")

#: The scene vocabulary, emitted as a small TBox with every export.
TERMS: Sequence[Tuple[str, str, str, str]] = (
    ("Object", "class", "3D object", "A object in a 3D scene, with a transform and geometry."),
    ("Scene", "class", "3D scene", "A collection of 3D objects authored together."),
    ("Collection", "class", "collection", "A named grouping of objects within a scene."),
    ("inScene", "object", "in scene", "The scene this object belongs to."),
    ("inCollection", "object", "in collection", "A collection this object belongs to."),
    ("parent", "object", "parent object", "The object this one is parented to."),
    ("material", "object", "material", "The material assigned to this object."),
    ("locationX", "data", "location X", "World-space X position, in metres."),
    ("locationY", "data", "location Y", "World-space Y position, in metres."),
    ("locationZ", "data", "location Z", "World-space Z position, in metres."),
    ("rotationX", "data", "rotation X", "World-space X Euler rotation, in radians (XYZ order)."),
    ("rotationY", "data", "rotation Y", "World-space Y Euler rotation, in radians (XYZ order)."),
    ("rotationZ", "data", "rotation Z", "World-space Z Euler rotation, in radians (XYZ order)."),
    ("scaleX", "data", "scale X", "Scale factor on X."),
    ("scaleY", "data", "scale Y", "Scale factor on Y."),
    ("scaleZ", "data", "scale Z", "Scale factor on Z."),
    ("dimensionX", "data", "width", "Bounding-box extent on X, in metres."),
    ("dimensionY", "data", "depth", "Bounding-box extent on Y, in metres."),
    ("dimensionZ", "data", "height", "Bounding-box extent on Z, in metres."),
    ("objectType", "data", "object type", "Blender object type: MESH, EMPTY, CURVE, LIGHT, CAMERA, …"),
    ("vertexCount", "data", "vertex count", "Number of mesh vertices."),
    ("faceCount", "data", "face count", "Number of mesh faces."),
    ("baseColor", "data", "base colour", "The material's base colour as an sRGB hex string."),
    ("frameStart", "data", "first frame", "First keyframe of this object's animation."),
    ("frameEnd", "data", "last frame", "Last keyframe of this object's animation."),
    ("visible", "data", "visible", "Whether the object is visible in renders."),
)


def escape(text: str) -> str:
    return (
        text.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
    )


class Writer:
    """Accumulates N-Triples lines."""

    def __init__(self):
        self.lines: List[str] = []

    def iri(self, s: str, p: str, o: str) -> None:
        self.lines.append(f"<{s}> <{p}> <{o}> .")

    def text(self, s: str, p: str, value: str, *, lang: str = "", datatype: str = "") -> None:
        literal = f'"{escape(value)}"'
        if lang:
            literal += f"@{lang}"
        elif datatype:
            literal += f"^^<{datatype}>"
        self.lines.append(f"<{s}> <{p}> {literal} .")

    def number(self, s: str, p: str, value: float, *, integer: bool = False) -> None:
        if value is None or (isinstance(value, float) and not math.isfinite(value)):
            return
        if integer:
            self.text(s, p, str(int(value)), datatype=XSD + "integer")
        else:
            self.text(s, p, f"{float(value):.6g}", datatype=XSD + "double")

    def boolean(self, s: str, p: str, value: bool) -> None:
        self.text(s, p, "true" if value else "false", datatype=XSD + "boolean")

    def dump(self) -> str:
        return "\n".join(self.lines) + "\n"


def mint(base: str, name: str, used: Dict[str, int]) -> str:
    """A stable IRI for an object that never had one."""
    slug = _IRI_SAFE_RE.sub("-", name).strip("-").lower() or "object"
    count = used.get(slug, 0)
    used[slug] = count + 1
    return f"{base}{slug}" if count == 0 else f"{base}{slug}-{count}"


def _hex_color(rgba: Sequence[float]) -> str:
    def to_srgb(c: float) -> int:
        c = max(0.0, min(1.0, c))
        s = 12.92 * c if c <= 0.0031308 else 1.055 * (c ** (1 / 2.4)) - 0.055
        return int(round(s * 255))

    return "#%02x%02x%02x" % tuple(to_srgb(c) for c in rgba[:3])


def _base_color(obj: "bpy.types.Object") -> Optional[str]:
    for slot in obj.material_slots:
        mat = slot.material
        if mat is None:
            continue
        if mat.use_nodes:
            for node in mat.node_tree.nodes:
                if node.type == "BSDF_PRINCIPLED":
                    socket = node.inputs.get("Base Color")
                    if socket is not None:
                        return _hex_color(socket.default_value)
        return _hex_color(mat.diffuse_color)
    return None


def _frame_range(obj: "bpy.types.Object") -> Optional[Tuple[float, float]]:
    from . import timeline

    anim = obj.animation_data
    if not anim or not anim.action:
        return None
    frames = [
        kp.co.x
        for curve in timeline.iter_fcurves(anim.action)
        for kp in curve.keyframe_points
    ]
    return (min(frames), max(frames)) if frames else None


def _emit_terms(w: Writer) -> None:
    kinds = {
        "class": OWL + "Class",
        "object": OWL + "ObjectProperty",
        "data": OWL + "DatatypeProperty",
    }
    for local, kind, label, comment in TERMS:
        iri = SCENE + local
        w.iri(iri, RDF + "type", kinds[kind])
        w.text(iri, RDFS + "label", label, lang="en")
        w.text(iri, RDFS + "comment", comment, lang="en")


def scene_to_ntriples(
    objects: Sequence["bpy.types.Object"],
    *,
    base: str,
    scene_name: str = "scene",
    keep_iris: bool = True,
    include_terms: bool = True,
    include_transforms: bool = True,
) -> Tuple[str, int]:
    """Serialise objects as N-Triples. Returns ``(text, subject_count)``."""
    w = Writer()
    if include_terms:
        _emit_terms(w)

    if not base.endswith(("/", "#", ":")):
        base += "/"
    scene_iri = f"{base}{_IRI_SAFE_RE.sub('-', scene_name).strip('-').lower() or 'scene'}"
    w.iri(scene_iri, RDF + "type", SCENE + "Scene")
    w.text(scene_iri, RDFS + "label", scene_name, lang="en")

    used: Dict[str, int] = {}
    iri_of: Dict[str, str] = {}
    for obj in objects:
        existing = rprops.iri_of(obj) if keep_iris else ""
        iri_of[obj.name] = existing or mint(base, obj.name, used)

    for obj in objects:
        subject = iri_of[obj.name]
        w.iri(subject, RDF + "type", SCENE + "Object")
        w.iri(subject, SCENE + "inScene", scene_iri)
        w.text(subject, RDFS + "label", obj.name)
        w.text(subject, SCENE + "objectType", obj.type)

        # The classes the object carried in from its source graph.
        for klass in rprops.classes_of(obj):
            w.iri(subject, RDF + "type", klass)

        if include_transforms:
            loc = obj.matrix_world.translation
            rot = obj.matrix_world.to_euler("XYZ")
            scl = obj.matrix_world.to_scale()
            for axis, index in (("X", 0), ("Y", 1), ("Z", 2)):
                w.number(subject, SCENE + f"location{axis}", loc[index])
                w.number(subject, SCENE + f"rotation{axis}", rot[index])
                w.number(subject, SCENE + f"scale{axis}", scl[index])
                w.number(subject, SCENE + f"dimension{axis}", obj.dimensions[index])
            # A geometry literal too, so the export is spatially queryable.
            w.text(
                subject,
                GEO3 + "asWKT3D",
                f"POINT Z({loc[0]:.6g} {loc[1]:.6g} {loc[2]:.6g})",
                datatype=WKT3D_DT,
            )
            w.text(
                subject,
                GEO + "asWKT",
                f"POINT({loc[0]:.6g} {loc[1]:.6g})",
                datatype=GEO + "wktLiteral",
            )

        if obj.parent is not None and obj.parent.name in iri_of:
            w.iri(subject, SCENE + "parent", iri_of[obj.parent.name])
        for coll in obj.users_collection:
            w.iri(subject, SCENE + "inCollection", f"{base}collection/{_IRI_SAFE_RE.sub('-', coll.name).lower()}")

        if obj.type == "MESH" and obj.data is not None:
            w.number(subject, SCENE + "vertexCount", len(obj.data.vertices), integer=True)
            w.number(subject, SCENE + "faceCount", len(obj.data.polygons), integer=True)

        color = _base_color(obj)
        if color:
            w.text(subject, SCENE + "baseColor", color)

        frames = _frame_range(obj)
        if frames:
            w.number(subject, SCENE + "frameStart", frames[0])
            w.number(subject, SCENE + "frameEnd", frames[1])

        w.boolean(subject, SCENE + "visible", not obj.hide_render)

        _emit_custom_properties(w, obj, subject, base)

    for coll in {c for obj in objects for c in obj.users_collection}:
        coll_iri = f"{base}collection/{_IRI_SAFE_RE.sub('-', coll.name).lower()}"
        w.iri(coll_iri, RDF + "type", SCENE + "Collection")
        w.text(coll_iri, RDFS + "label", coll.name)

    return (w.dump(), len(objects))


def _emit_custom_properties(w: Writer, obj: "bpy.types.Object", subject: str, base: str) -> None:
    """Custom properties back to statements, restoring the original predicates."""
    predicates = rprops.predicate_map(obj)
    datatypes = rprops.datatype_map(obj)
    for key in rprops.user_keys(obj):
        predicate = predicates.get(key) or f"{base}prop/{key}"
        datatype = datatypes.get(key, "")
        for value in rprops.values_of(obj, key):
            if isinstance(value, bool):
                w.boolean(subject, predicate, value)
            elif isinstance(value, (int, float)):
                w.number(subject, predicate, float(value), integer=isinstance(value, int))
            elif isinstance(value, str):
                if value.startswith(("http://", "https://", "urn:")) and " " not in value:
                    w.iri(subject, predicate, value)
                else:
                    w.text(subject, predicate, value, datatype=datatype)
            elif isinstance(value, (list, tuple)):
                for item in value:
                    if isinstance(item, (int, float)):
                        w.number(subject, predicate, float(item))


def build_rete(
    ntriples: str,
    path: str,
    *,
    title: str = "Blender scene",
    description: str = "",
    license_: str = "",
    text_index: bool = True,
) -> Dict[str, object]:
    """Build a ``.rete`` file from the exported triples. Returns build stats."""
    mod = engine.engine()
    if mod is None:
        raise RuntimeError(engine.unavailable_reason())

    builder = mod.Builder().add(ntriples, "nt")
    builder.card(
        title=title,
        description=description
        or "A 3D scene exported from Blender by the rete add-on: objects, "
        "transforms, hierarchy, materials and their inherited RDF properties.",
        license=license_ or "CC0-1.0",
        source="blender",
    )
    builder.example(
        "PREFIX scene: <https://w3id.org/rete/scene#>\n"
        "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n"
        "SELECT ?label ?x ?y ?z WHERE {\n"
        "  ?o a scene:Object ; rdfs:label ?label ;\n"
        "     scene:locationX ?x ; scene:locationY ?y ; scene:locationZ ?z .\n"
        "} ORDER BY ?label",
        title="Every object and where it sits",
        question="What is in this scene, and where is each thing?",
    )
    builder.example(
        "PREFIX scene: <https://w3id.org/rete/scene#>\n"
        "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n"
        "SELECT ?child ?parent WHERE {\n"
        "  ?c scene:parent ?p ; rdfs:label ?child .\n"
        "  ?p rdfs:label ?parent .\n"
        "}",
        title="The object hierarchy",
        question="Which objects are parented to which?",
    )
    if text_index:
        builder.text_index()
    builder.export(path)
    return dict(builder.stats or {})


def collect_objects(
    context,
    *,
    scope: str = "SCENE",
    collection_name: str = "",
) -> List["bpy.types.Object"]:
    """The objects an export should cover."""
    if scope == "SELECTED":
        return list(context.selected_objects)
    if scope == "COLLECTION" and collection_name:
        coll = bpy.data.collections.get(collection_name)
        return list(coll.all_objects) if coll else []
    if scope == "GRAPH":
        return [o for o in context.scene.objects if o.get(rprops.IRI)]
    return list(context.scene.objects)
