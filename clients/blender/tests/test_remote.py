"""End-to-end tests against the published graphs. Needs network.

    docker run --rm -v "$PWD":/work -w /work rete-blender \
        blender -b --factory-startup -noaudio \
            --python clients/blender/tests/test_remote.py

Each case is a real dataset read lazily over HTTP range requests, so this also
checks the thing that is easy to get wrong in a synthetic fixture: that the
column heuristics fire correctly on vocabularies written by other people.
"""

from __future__ import annotations

import os
import sys
import traceback

import bpy

HERE = os.path.dirname(os.path.abspath(__file__))
ADDON_ROOT = os.path.dirname(HERE)
if ADDON_ROOT not in sys.path:
    sys.path.insert(0, ADDON_ROOT)

from addon import builder, detect, engine, props as rprops  # noqa: E402

PASSED, FAILED = [], []

ANATOMY = "https://data.graphplaza.com/z-anatomy/z-anatomy.rete"
SMITHSONIAN = "https://data.graphplaza.com/smithsonian3d/smithsonian3d.rete"
DANCE = "https://data.graphplaza.com/dance/dance.rete"
TRACKING = "https://data.graphplaza.com/tracking/tracking.rete"


def test(name):
    def wrap(fn):
        def run():
            try:
                fn()
                PASSED.append(name)
                print(f"  ok   {name}")
            except Exception as exc:
                FAILED.append((name, exc))
                print(f"  FAIL {name}: {exc}")
                traceback.print_exc()
        return run
    return wrap


def eq(actual, expected, what=""):
    if actual != expected:
        raise AssertionError(f"{what or 'value'}: expected {expected!r}, got {actual!r}")


def truthy(value, what=""):
    if not value:
        raise AssertionError(f"{what or 'value'} is falsy: {value!r}")


def fresh(name: str):
    """A clean collection per case, so counts are unambiguous."""
    existing = bpy.data.collections.get(name)
    if existing is not None:
        for obj in list(existing.all_objects):
            bpy.data.objects.remove(obj, do_unlink=True)
    return name


@test("z-anatomy: the card, the schema and a lazy query")
def t_anatomy_open():
    card = engine.card(ANATOMY)
    truthy(card.get("title"), "card title present")
    print(f"       {card.get('title')} — {card.get('license')}")
    stats = engine.stats(ANATOMY)
    megabytes = stats.get("bytes", 0) / 1e6
    truthy(megabytes < 6.0, f"opening read only {megabytes:.2f} MB of a remote file")


@test("z-anatomy: 3D structures land in the right place, at the right size")
def t_anatomy_geometry():
    query = """
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geo3: <https://w3id.org/rete/geo3#>
PREFIX anat: <https://w3id.org/rete/anatomy#>
SELECT ?s ?label ?wkt ?box ?tissue WHERE {
  ?s a anat:AnatomicalStructure ; rdfs:label ?label ;
     geo:hasGeometry ?g ; anat:tissueType ?tissue .
  ?g geo3:asWKT3D ?wkt ; geo3:box ?box .
  FILTER(langMatches(lang(?label), "en"))
} LIMIT 60
"""
    result = engine.select(ANATOMY, query)
    truthy(len(result) > 10, f"rows returned ({len(result)})")

    roles = detect.classify_result(result)
    eq(roles["wkt"], detect.GEOMETRY, "asWKT3D detected as geometry")
    eq(roles["box"], detect.GEOMETRY, "box detected as geometry")
    eq(roles["s"], detect.ENTITY)
    eq(roles["label"], detect.LABEL)

    name = fresh("remote-anatomy")
    report = builder.build(
        result,
        builder.Settings(
            source=ANATOMY,
            query=query,
            collection_name=name,
            scale_mode="MM",       # the anatomy graph is authored in millimetres
            flip_x=True,           # +X is the subject's left; Blender's is right
            recentre=True,
            deep_properties=True,
            material_mode="AUTO",
            layout="GEOMETRY",
            import_assets=False,
        ),
    )
    truthy(report.objects > 10, f"objects built ({report.objects})")
    truthy(report.properties > report.objects, "properties inherited from the graph")

    objects = list(bpy.data.collections[name].all_objects)
    bpy.context.view_layer.update()
    # A human body is roughly 1.8 m tall: millimetre coordinates scaled by 1/1000
    # must land in that ballpark, not 1800 m or 1.8 mm.
    span = max(o.matrix_world.translation[2] for o in objects) - min(
        o.matrix_world.translation[2] for o in objects
    )
    truthy(0.05 < span < 2.5, f"vertical span is human-scaled ({span:.3f} m)")
    sized = [o for o in objects if max(o.dimensions) > 1e-4]
    truthy(sized, "bounding boxes sized the markers")
    truthy(
        max(max(o.dimensions) for o in sized) < 1.0,
        "no structure is larger than a metre",
    )

    sample = objects[0]
    truthy(rprops.iri_of(sample).startswith("http"), "identity stamped")
    truthy(len(rprops.user_keys(sample)) > 3, "several inherited properties")
    print(f"       {report.summary()}")
    print(f"       sample: {sample.name} — {len(rprops.user_keys(sample))} properties")


@test("z-anatomy: a shared body-system .glb is imported once and nodes linked")
def t_anatomy_assets():
    query = """
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX anat: <https://w3id.org/rete/anatomy#>
SELECT ?s ?label ?glb ?node WHERE {
  ?s rdfs:label ?label ; anat:glbFile ?glb ; anat:meshNode ?node ;
     anat:inSystem <https://w3id.org/rete/anatomy/system/skel> .
  FILTER(langMatches(lang(?label), "en"))
} LIMIT 12
"""
    result = engine.select(ANATOMY, query)
    truthy(len(result) > 3, f"rows returned ({len(result)})")

    roles = detect.classify_result(result)
    eq(roles["glb"], detect.ASSET, "glbFile detected as an asset (it is a plain literal)")
    eq(roles["node"], detect.MESH_NODE, "meshNode detected")

    name = fresh("remote-anatomy-mesh")
    report = builder.build(
        result,
        builder.Settings(
            source=ANATOMY,
            query=query,
            collection_name=name,
            deep_properties=False,
            material_mode="CLASS",
            import_assets=True,
            max_assets=12,
        ),
    )
    truthy(report.assets > 3, f"assets instanced ({report.assets})")

    objects = [o for o in bpy.data.collections[name].all_objects if o.type == "MESH"]
    truthy(objects, "mesh objects created")
    truthy(all(len(o.data.vertices) > 0 for o in objects), "every node has geometry")

    # The whole point of the shared-asset path: one import, many linked copies.
    meshes = {o.data.name for o in objects}
    truthy(
        len(meshes) == len(objects),
        "each structure is its own node inside the shared file",
    )
    bones = [o for o in objects if max(o.dimensions) > 0]
    truthy(bones, "imported bones have real dimensions")
    print(f"       {report.summary()} — {len(objects)} skeletal nodes")


@test("smithsonian3d: standalone .glb models import and lay out")
def t_smithsonian():
    query = """
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX p: <https://3d.si.edu/prop/>
SELECT ?o ?label ?mesh WHERE {
  ?o rdfs:label ?label ; p:mesh ?mesh .
} LIMIT 3
"""
    result = engine.select(SMITHSONIAN, query)
    eq(len(result), 3, "rows")
    roles = detect.classify_result(result)
    eq(roles["mesh"], detect.ASSET, "prop:mesh detected as an asset")

    name = fresh("remote-smithsonian")
    report = builder.build(
        result,
        builder.Settings(
            source=SMITHSONIAN,
            query=query,
            collection_name=name,
            deep_properties=True,
            import_assets=True,
            max_assets=3,
            layout="GRID",
            layout_spacing=2.0,
            material_mode="NONE",
        ),
    )
    truthy(report.assets >= 1, f"models imported ({report.assets}); {report.warnings}")
    meshes = [o for o in bpy.data.collections[name].all_objects if o.type == "MESH"]
    truthy(meshes, "meshes in the scene")
    truthy(sum(len(o.data.vertices) for o in meshes) > 1000, "real geometry arrived")
    print(f"       {report.summary()}")


@test("dance: an animated skeleton .glb keeps its animation")
def t_dance():
    query = """
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX dance: <https://w3id.org/rete/dance#>
SELECT ?perf ?label ?animation WHERE {
  ?perf rdfs:label ?label ; dance:animation ?animation .
} LIMIT 1
"""
    result = engine.select(DANCE, query)
    eq(len(result), 1, "one performance")
    roles = detect.classify_result(result)
    eq(roles["animation"], detect.ASSET, "dance:animation detected as an asset")

    name = fresh("remote-dance")
    report = builder.build(
        result,
        builder.Settings(
            source=DANCE,
            query=query,
            collection_name=name,
            deep_properties=False,
            import_assets=True,
            max_assets=1,
            material_mode="NONE",
        ),
    )
    truthy(report.assets == 1, f"animation imported; {report.warnings}")

    objects = list(bpy.data.collections[name].all_objects)
    animated = [o for o in objects if o.animation_data and o.animation_data.action]
    truthy(
        animated or any(a.users for a in bpy.data.actions),
        "the .glb brought animation data with it",
    )
    print(f"       {len(objects)} objects, {len(bpy.data.actions)} actions in the file")


@test("tracking: positions over time become moving objects")
def t_tracking():
    query = """
PREFIX tr: <https://w3id.org/rete/tracking#>
SELECT ?pos ?object ?t ?x ?y WHERE {
  ?pos tr:object ?object ; tr:t ?t ; tr:x ?x ; tr:y ?y .
  FILTER(?t < 4.0)
} ORDER BY ?t LIMIT 300
"""
    result = engine.select(TRACKING, query)
    truthy(len(result) > 50, f"samples returned ({len(result)})")

    roles = detect.classify_result(result)
    eq(roles["t"], detect.TIME, "tr:t recognised as the time axis")
    eq(roles["x"], detect.NUMBER)
    binding = detect.resolve(result, roles, {})
    eq(binding.xyz, ["x", "y", ""], "x/y read as coordinates")

    name = fresh("remote-tracking")
    report = builder.build(
        result,
        builder.Settings(
            source=TRACKING,
            query=query,
            collection_name=name,
            time_mode="PATH",
            frame_start=1,
            frame_end=100,
            scale_mode="M",
            recentre=True,
            deep_properties=False,
            material_mode="NONE",
            point_size=0.4,
        ),
    )
    # 22 players plus the ball, not one object per sample.
    truthy(1 < report.objects < len(result), f"grouped into {report.objects} moving objects")
    truthy(report.keyframed > 1, f"objects keyframed ({report.keyframed})")

    objects = list(bpy.data.collections[name].all_objects)
    mover = next(o for o in objects if o.animation_data and o.animation_data.action)
    scene = bpy.context.scene
    scene.frame_set(1)
    start = mover.matrix_world.translation.copy()
    scene.frame_set(100)
    moved = (mover.matrix_world.translation - start).length
    truthy(moved > 1e-4, f"the object moves over the timeline ({moved:.3f} m)")
    scene.frame_set(1)
    print(f"       {report.summary()}, moved {moved:.2f} m")


def main() -> None:
    print("\nrete Blender add-on — remote dataset tests")
    print(f"Blender {bpy.app.version_string}, engine {engine.version() or 'MISSING'}\n")
    if not engine.available():
        print("engine unavailable:", engine.unavailable_reason())
        sys.exit(1)

    for fn in (
        t_anatomy_open, t_anatomy_geometry, t_anatomy_assets,
        t_smithsonian, t_dance, t_tracking,
    ):
        fn()

    total = 0.0
    for source in (ANATOMY, SMITHSONIAN, DANCE, TRACKING):
        total += engine.stats(source).get("bytes", 0)
    print(f"\ntotal fetched from all four graphs: {total / 1e6:.2f} MB")
    print(f"{len(PASSED)} passed, {len(FAILED)} failed")
    if FAILED:
        for name, exc in FAILED:
            print(f"  FAILED {name}: {exc}")
        sys.exit(1)
    print("ALL REMOTE TESTS PASSED")


if __name__ == "__main__":
    main()
