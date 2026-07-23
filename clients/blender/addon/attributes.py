"""A Geometry Nodes bridge: query results as an attributed point cloud.

Importing one object per row is the right thing for a few thousand rows and the
wrong thing for a few hundred thousand. This module writes the result set into a
single mesh — one vertex per row, every numeric and colour column as a named
attribute — which Geometry Nodes can then instance on, scale by, colour by and
filter, at a scale objects cannot reach.

String columns cannot be attributes (Geometry Nodes has no string field), so
they are encoded as an integer category index, with the index-to-value table
stored on the object so the mapping stays legible and round-trips.
"""

from __future__ import annotations

import json
from typing import Dict, List, Optional, Sequence, Tuple

import bpy

from . import detect, materials, props as rprops

#: Custom property holding ``{attribute: [values]}`` for categorical columns.
CATEGORIES = "rete:categories"


def _new_attribute(mesh: "bpy.types.Mesh", name: str, data_type: str):
    existing = mesh.attributes.get(name)
    if existing is not None:
        mesh.attributes.remove(existing)
    return mesh.attributes.new(name=name, type=data_type, domain="POINT")


def build_point_cloud(
    name: str,
    positions: Sequence[Tuple[float, float, float]],
    columns: Dict[str, Sequence],
    collection: "bpy.types.Collection",
    *,
    roles: Optional[Dict[str, str]] = None,
) -> "bpy.types.Object":
    """A vertex-per-row mesh carrying every column as a point attribute.

    ``columns`` maps a variable name to that column's cells (``None`` for
    unbound). ``roles`` is the detected role per column, which decides whether a
    column becomes a float, a colour or a category.
    """
    roles = roles or {}
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(list(positions), [], [])
    mesh.update()

    count = len(positions)
    categories: Dict[str, List[str]] = {}

    for var, cells in columns.items():
        key = rprops.prop_key(var) or var
        role = roles.get(var, detect.TEXT)
        cells = list(cells)[:count]
        cells += [None] * (count - len(cells))

        if role == detect.NUMBER:
            values: List[float] = []
            for cell in cells:
                number = cell.as_number() if cell is not None else None
                values.append(float(number) if number is not None else 0.0)
            attribute = _new_attribute(mesh, key, "FLOAT")
            attribute.data.foreach_set("value", values)

        elif role == detect.COLOR:
            flat: List[float] = []
            for cell in cells:
                rgba = materials.parse_color(cell.value) if cell is not None else None
                flat.extend(rgba or (0.5, 0.5, 0.5, 1.0))
            attribute = _new_attribute(mesh, key, "FLOAT_COLOR")
            attribute.data.foreach_set("color", flat)

        elif role in (detect.TIME, detect.TIME_END):
            from . import timeline

            values = []
            for cell in cells:
                seconds = timeline.to_seconds(cell.value, cell.datatype) if cell else None
                values.append(float(seconds) if seconds is not None else 0.0)
            attribute = _new_attribute(mesh, key, "FLOAT")
            attribute.data.foreach_set("value", values)

        elif role in (detect.CLASS, detect.TEXT, detect.LABEL, detect.ENTITY):
            # Categorical: an int index per point, plus the lookup table.
            table: List[str] = []
            lookup: Dict[str, int] = {}
            indices: List[int] = []
            for cell in cells:
                if cell is None:
                    indices.append(-1)
                    continue
                value = cell.value
                if value not in lookup:
                    lookup[value] = len(table)
                    table.append(value)
                indices.append(lookup[value])
            if len(table) <= 1 and role in (detect.TEXT, detect.ENTITY):
                continue  # nothing to distinguish points by
            attribute = _new_attribute(mesh, key, "INT")
            attribute.data.foreach_set("value", indices)
            categories[key] = table

    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    if categories:
        obj[CATEGORIES] = json.dumps(categories, ensure_ascii=False)
    return obj


def category_table(obj: "bpy.types.Object") -> Dict[str, List[str]]:
    """The index-to-value tables for an attributed point cloud."""
    raw = obj.get(CATEGORIES)
    if not raw:
        return {}
    try:
        value = json.loads(raw)
        return value if isinstance(value, dict) else {}
    except ValueError:
        return {}


def add_instancer(
    points: "bpy.types.Object",
    *,
    scale_attribute: str = "",
    color_attribute: str = "",
    radius: float = 0.05,
) -> Optional["bpy.types.NodesModifier"]:
    """Attach a Geometry Nodes tree that instances a sphere on every point.

    A working starting point rather than a finished look: the tree is a plain
    instance-on-points graph with optional scale and colour wired from named
    attributes, left open for the user to take further.
    """
    modifier = points.modifiers.new(name="rete instances", type="NODES")
    tree = bpy.data.node_groups.new("rete instancer", "GeometryNodeTree")
    modifier.node_group = tree

    # Blender 4.0 replaced the group input/output sockets API.
    if hasattr(tree, "interface"):
        tree.interface.new_socket("Geometry", in_out="INPUT", socket_type="NodeSocketGeometry")
        tree.interface.new_socket("Geometry", in_out="OUTPUT", socket_type="NodeSocketGeometry")
    else:  # pragma: no cover - Blender 3.x
        tree.inputs.new("NodeSocketGeometry", "Geometry")
        tree.outputs.new("NodeSocketGeometry", "Geometry")

    nodes, links = tree.nodes, tree.links
    group_in = nodes.new("NodeGroupInput")
    group_in.location = (-600, 0)
    group_out = nodes.new("NodeGroupOutput")
    group_out.location = (600, 0)

    instance = nodes.new("GeometryNodeInstanceOnPoints")
    instance.location = (100, 0)
    sphere = nodes.new("GeometryNodeMeshIcoSphere")
    sphere.location = (-200, -220)
    sphere.inputs["Radius"].default_value = radius
    sphere.inputs["Subdivisions"].default_value = 2

    links.new(group_in.outputs[0], instance.inputs["Points"])
    links.new(sphere.outputs["Mesh"], instance.inputs["Instance"])

    tail = instance
    if scale_attribute:
        named = nodes.new("GeometryNodeInputNamedAttribute")
        named.location = (-200, 120)
        named.data_type = "FLOAT"
        named.inputs["Name"].default_value = scale_attribute
        # Map the raw value into a sane instance-scale band.
        map_range = nodes.new("ShaderNodeMapRange")
        map_range.location = (-40, 160)
        map_range.inputs["To Min"].default_value = 0.3
        map_range.inputs["To Max"].default_value = 3.0
        links.new(named.outputs[0], map_range.inputs["Value"])
        links.new(map_range.outputs["Result"], instance.inputs["Scale"])

    if color_attribute:
        realize = nodes.new("GeometryNodeRealizeInstances")
        realize.location = (300, 0)
        links.new(instance.outputs["Instances"], realize.inputs["Geometry"])
        store = nodes.new("GeometryNodeStoreNamedAttribute")
        store.location = (440, 0)
        store.data_type = "FLOAT_COLOR"
        store.domain = "POINT"
        store.inputs["Name"].default_value = color_attribute
        named_color = nodes.new("GeometryNodeInputNamedAttribute")
        named_color.location = (300, -220)
        named_color.data_type = "FLOAT_COLOR"
        named_color.inputs["Name"].default_value = color_attribute
        links.new(realize.outputs["Geometry"], store.inputs["Geometry"])
        links.new(named_color.outputs[0], store.inputs["Value"])
        tail = store
    links.new(tail.outputs[0], group_out.inputs[0])
    return modifier
