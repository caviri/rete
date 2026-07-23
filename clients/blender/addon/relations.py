"""Relations between entities, expressed as scene structure.

One predicate can be read three ways, and the add-on offers all three because
they answer different questions:

* **Parenting** — ``partOf`` becomes Blender's object hierarchy, so moving a
  body system moves its organs and the Outliner mirrors the partonomy.
* **Collections** — the same relation as grouping, when you want to toggle
  whole branches rather than transform them.
* **Edges** — the relation drawn as actual geometry, one line per statement, so
  the graph's shape is visible in 3D (and can be given a material, extruded
  into tubes, or fed to Geometry Nodes).
"""

from __future__ import annotations

from typing import Dict, Iterable, List, Optional, Sequence, Tuple

import bpy

from . import engine, props as rprops

MODES = (
    ("PARENT", "Parent objects", "The relation becomes Blender's object hierarchy"),
    ("COLLECTION", "Collections", "Each parent becomes a collection holding its children"),
    ("EDGES", "Edge geometry", "Each statement becomes a line between the two objects"),
    ("NONE", "None", "Do not apply the relation to the scene"),
)


def fetch_edges(
    source: str,
    objects: Dict[str, "bpy.types.Object"],
    predicate: str,
    *,
    inverse: bool = False,
    within_scene: bool = True,
) -> List[Tuple["bpy.types.Object", "bpy.types.Object", str]]:
    """Statements over the imported entities, as ``(from, to, target_iri)``.

    With ``within_scene`` the target must also be in the scene — the usual case,
    since an edge to something that was never imported has nowhere to attach.
    """
    if not predicate or not objects:
        return []
    pairs = engine.pairs_by_predicate(source, list(objects.keys()), predicate, inverse=inverse)
    edges: List[Tuple["bpy.types.Object", "bpy.types.Object", str]] = []
    for subject_iri, target in pairs:
        subject = objects.get(subject_iri)
        if subject is None:
            continue
        target_obj = objects.get(target.value) if target.is_iri else None
        if target_obj is None and within_scene:
            continue
        edges.append((subject, target_obj, target.value))
    return edges


def apply_parenting(
    edges: Sequence[Tuple["bpy.types.Object", Optional["bpy.types.Object"], str]],
) -> int:
    """Parent each subject to its target, keeping world transforms intact.

    Cycles are refused rather than resolved: a graph can legitimately state
    mutual containment, and Blender crashes on a parent loop.
    """
    made = 0
    for child, parent, _ in edges:
        if parent is None or child is parent or _would_cycle(child, parent):
            continue
        world = child.matrix_world.copy()
        child.parent = parent
        child.matrix_parent_inverse = parent.matrix_world.inverted()
        child.matrix_world = world
        made += 1
    return made


def _would_cycle(child: "bpy.types.Object", parent: "bpy.types.Object") -> bool:
    node = parent
    seen = 0
    while node is not None and seen < 1000:
        if node is child:
            return True
        node = node.parent
        seen += 1
    return False


def apply_collections(
    edges: Sequence[Tuple["bpy.types.Object", Optional["bpy.types.Object"], str]],
    root: "bpy.types.Collection",
    labels: Optional[Dict[str, str]] = None,
) -> int:
    """Group children into a collection named after each parent."""
    labels = labels or {}
    made = 0
    for child, parent, target_iri in edges:
        name = labels.get(target_iri) or (parent.name if parent else rprops.local_name(target_iri))
        group = bpy.data.collections.get(name)
        if group is None:
            group = bpy.data.collections.new(name)
            root.children.link(group)
        if child.name not in group.objects:
            for existing in list(child.users_collection):
                existing.objects.unlink(child)
            group.objects.link(child)
            made += 1
    return made


def build_edge_mesh(
    edges: Sequence[Tuple["bpy.types.Object", Optional["bpy.types.Object"], str]],
    name: str,
    collection: "bpy.types.Collection",
) -> Optional["bpy.types.Object"]:
    """One mesh holding every edge as a line segment.

    A single mesh rather than an object per edge: a relation with a hundred
    thousand statements is then one object Blender can draw, and a Skin or
    Wireframe modifier turns it into solid tubes in one step.
    """
    segments = [(a, b) for a, b, _ in edges if b is not None]
    if not segments:
        return None

    verts: List[Tuple[float, float, float]] = []
    edge_indices: List[Tuple[int, int]] = []
    index: Dict[str, int] = {}
    for a, b in segments:
        for obj in (a, b):
            if obj.name not in index:
                index[obj.name] = len(verts)
                verts.append(tuple(obj.matrix_world.translation))
        edge_indices.append((index[a.name], index[b.name]))

    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(verts, edge_indices, [])
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    return obj


def apply(
    mode: str,
    source: str,
    objects: Dict[str, "bpy.types.Object"],
    predicate: str,
    root: "bpy.types.Collection",
    *,
    inverse: bool = False,
) -> Tuple[int, str]:
    """Fetch a relation and apply it in the chosen mode. Returns ``(n, note)``.

    Parenting and edges need both ends in the scene; grouping only needs the
    target as a key (the storey an element sits in names its collection even
    when no storey object was imported), so it keeps edges to outside targets.
    """
    if mode == "NONE" or not predicate:
        return (0, "")
    within = mode != "COLLECTION"
    edges = fetch_edges(source, objects, predicate, inverse=inverse, within_scene=within)
    if not edges:
        return (0, f"no {rprops.local_name(predicate)} statements among the imported entities")
    if mode == "PARENT":
        return (apply_parenting(edges), "parented")
    if mode == "COLLECTION":
        labels = engine.labels_for(source, [target for _, _, target in edges])
        return (apply_collections(edges, root, labels), "grouped")
    if mode == "EDGES":
        obj = build_edge_mesh(edges, f"edges:{rprops.local_name(predicate)}", root)
        return ((len(edges) if obj else 0), "drawn")
    return (0, "")
