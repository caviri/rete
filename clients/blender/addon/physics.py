"""Physics from the graph.

Two distinct ideas live here.

The first is ordinary: give imported objects rigid bodies, with mass and
friction read from numeric properties, so a dataset can be dropped, stacked or
shaken.

The second is the interesting one. A knowledge graph's *relations* become
physical **constraints**: pick a predicate — anatomical adjacency, a building's
topology, a citation, a co-authorship — and every edge becomes a rigid-body
constraint between the two objects. The graph stops being a diagram and becomes
a structure that holds itself together, and you can then pull it apart and watch
what the topology actually does.
"""

from __future__ import annotations

from typing import Dict, Iterable, List, Optional, Sequence, Tuple

import bpy

CONSTRAINT_TYPES = (
    ("FIXED", "Fixed", "Rigid weld — the relation is unbreakable"),
    ("POINT", "Point", "Ball joint — the pair stays connected but can rotate"),
    ("GENERIC_SPRING", "Spring", "Springy link — the graph settles like a net"),
    ("HINGE", "Hinge", "Rotation about one axis"),
    ("SLIDER", "Slider", "Translation along one axis"),
)

BODY_TYPES = (
    ("ACTIVE", "Active", "Simulated — falls, collides, is pushed around"),
    ("PASSIVE", "Passive", "Immovable collider that others react to"),
)

SHAPES = (
    ("CONVEX_HULL", "Convex hull", "Fast and stable; good default for meshes"),
    ("MESH", "Mesh", "Exact shape; expensive, and only reliable for passive bodies"),
    ("BOX", "Box", "The object's bounding box"),
    ("SPHERE", "Sphere", "A sphere enclosing the object"),
    ("CAPSULE", "Capsule", "A capsule enclosing the object"),
)

CONSTRAINT_COLLECTION = "rete constraints"


def ensure_world(scene: "bpy.types.Scene") -> Optional["bpy.types.Collection"]:
    """Make sure the scene has a rigid-body world, and return its collection."""
    if scene.rigidbody_world is None:
        try:
            bpy.ops.rigidbody.world_add()
        except RuntimeError:
            return None
    world = scene.rigidbody_world
    if world is None:
        return None
    if world.collection is None:
        coll = bpy.data.collections.new("RigidBodyWorld")
        world.collection = coll
    if world.constraints is None:
        world.constraints = bpy.data.collections.new("RigidBodyConstraints")
    return world.collection


def add_bodies(
    objects: Sequence["bpy.types.Object"],
    *,
    body_type: str = "ACTIVE",
    shape: str = "CONVEX_HULL",
    mass: float = 1.0,
    friction: float = 0.5,
    bounciness: float = 0.0,
) -> int:
    """Give every mesh in ``objects`` a rigid body. Returns how many got one.

    Objects are linked straight into the rigid-body world collection rather than
    driven through the operator one at a time — the operator needs each object
    to become active and re-evaluates the scene on every call, which is
    unusable past a few hundred rows.
    """
    scene = bpy.context.scene
    collection = ensure_world(scene)
    if collection is None:
        return 0

    count = 0
    for obj in objects:
        if obj.type != "MESH":
            continue
        if obj.name not in collection.objects:
            collection.objects.link(obj)
        body = obj.rigid_body
        if body is None:
            # Some builds only materialise the settings through the operator.
            body = _fallback_add(obj)
            if body is None:
                continue
        body.type = body_type
        body.collision_shape = shape
        body.mass = max(0.001, mass)
        body.friction = friction
        body.restitution = bounciness
        count += 1
    return count


def _fallback_add(obj: "bpy.types.Object"):
    view_layer = bpy.context.view_layer
    previous = view_layer.objects.active
    try:
        view_layer.objects.active = obj
        bpy.ops.rigidbody.object_add()
    except RuntimeError:
        return None
    finally:
        view_layer.objects.active = previous
    return obj.rigid_body


def scale_masses(
    objects: Sequence["bpy.types.Object"],
    key: str,
    *,
    low: float = 0.1,
    high: float = 100.0,
) -> int:
    """Set each body's mass from a numeric custom property, normalised.

    The raw values in graphs span absurd ranges (vertex counts, citation counts,
    populations), so they are mapped onto a usable mass band instead of being
    used literally — a body of mass 10^6 next to one of mass 1 just explodes.
    """
    from . import props as rprops

    values: List[Tuple["bpy.types.Object", float]] = []
    for obj in objects:
        value = rprops.number_of(obj, key)
        if value is not None and obj.rigid_body is not None:
            values.append((obj, value))
    if not values:
        return 0
    lo = min(v for _, v in values)
    hi = max(v for _, v in values)
    span = hi - lo
    for obj, value in values:
        t = 0.5 if span < 1e-9 else (value - lo) / span
        obj.rigid_body.mass = low + t * (high - low)
    return len(values)


def constraint_network(
    edges: Iterable[Tuple["bpy.types.Object", "bpy.types.Object"]],
    *,
    constraint_type: str = "FIXED",
    collection_name: str = CONSTRAINT_COLLECTION,
    spring_stiffness: float = 40.0,
    spring_damping: float = 0.5,
    limit: int = 20000,
) -> int:
    """Turn graph edges into rigid-body constraints. Returns how many were made.

    Each constraint lives on an empty placed at the midpoint of the pair, which
    is both what Blender expects and a readable visualisation of the edge in the
    viewport.
    """
    scene = bpy.context.scene
    if ensure_world(scene) is None:
        return 0
    holder = scene.rigidbody_world.constraints
    if holder is None:
        holder = bpy.data.collections.new("RigidBodyConstraints")
        scene.rigidbody_world.constraints = holder

    display = bpy.data.collections.get(collection_name)
    if display is None:
        display = bpy.data.collections.new(collection_name)
        scene.collection.children.link(display)

    made = 0
    seen: set = set()
    for a, b in edges:
        if made >= limit:
            break
        if a is None or b is None or a is b:
            continue
        pair = tuple(sorted((a.name, b.name)))
        if pair in seen:
            continue
        seen.add(pair)

        empty = bpy.data.objects.new(f"link:{a.name}→{b.name}", None)
        empty.empty_display_type = "SPHERE"
        empty.empty_display_size = 0.05
        empty.location = (a.matrix_world.translation + b.matrix_world.translation) / 2.0
        display.objects.link(empty)
        if empty.name not in holder.objects:
            holder.objects.link(empty)

        constraint = empty.rigid_body_constraint
        if constraint is None:
            view_layer = bpy.context.view_layer
            previous = view_layer.objects.active
            try:
                view_layer.objects.active = empty
                bpy.ops.rigidbody.constraint_add()
            except RuntimeError:
                continue
            finally:
                view_layer.objects.active = previous
            constraint = empty.rigid_body_constraint
        if constraint is None:
            continue

        constraint.type = constraint_type
        constraint.object1 = a
        constraint.object2 = b
        if constraint_type == "GENERIC_SPRING":
            for axis in ("x", "y", "z"):
                setattr(constraint, f"use_spring_{axis}", True)
                setattr(constraint, f"spring_stiffness_{axis}", spring_stiffness)
                setattr(constraint, f"spring_damping_{axis}", spring_damping)
        made += 1
    return made


def clear(scene: "bpy.types.Scene") -> None:
    """Remove the rigid-body world and every constraint empty this add-on made."""
    display = bpy.data.collections.get(CONSTRAINT_COLLECTION)
    if display is not None:
        for obj in list(display.objects):
            bpy.data.objects.remove(obj, do_unlink=True)
        bpy.data.collections.remove(display)
    if scene.rigidbody_world is not None:
        try:
            bpy.ops.rigidbody.world_remove()
        except RuntimeError:
            pass
