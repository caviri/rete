"""Headless tests for the rete Blender add-on.

Run inside the container:

    docker run --rm -v "$PWD":/work -w /work rete-blender \
        blender -b --factory-startup -noaudio \
            --python clients/blender/tests/run_tests.py

Everything runs offline: the fixture graph is built in memory with the engine's
own Builder, and the 3D asset is exported from Blender itself, so the asset and
mesh-node paths are exercised without touching the network.
"""

from __future__ import annotations

import os
import sys
import tempfile
import traceback

import bpy

HERE = os.path.dirname(os.path.abspath(__file__))
ADDON_ROOT = os.path.dirname(HERE)
if ADDON_ROOT not in sys.path:
    sys.path.insert(0, ADDON_ROOT)

import addon  # noqa: E402
from addon import (  # noqa: E402
    assets,
    attributes,
    builder,
    detect,
    engine,
    export,
    geometry,
    materials,
    physics,
    props as rprops,
    relations,
    timeline,
)

REPO = os.path.dirname(os.path.dirname(ADDON_ROOT))

PASSED: list = []
FAILED: list = []


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
        run.__name__ = fn.__name__
        return run

    return wrap


def eq(actual, expected, what=""):
    if actual != expected:
        raise AssertionError(f"{what or 'value'}: expected {expected!r}, got {actual!r}")


def fresh_local(name: str) -> str:
    """A clean collection name, so re-runs in one session don't accumulate."""
    existing = bpy.data.collections.get(name)
    if existing is not None:
        for obj in list(existing.all_objects):
            bpy.data.objects.remove(obj, do_unlink=True)
    return name


def _pairs(source, iris, predicate):
    """``[(subject_iri, object_iri)]`` for one predicate over the given IRIs."""
    out = []
    for subject, cell in engine.pairs_by_predicate(source, iris, predicate):
        if cell.is_iri:
            out.append((subject, cell.value))
    return out


def close(actual, expected, tol=1e-4, what=""):
    if abs(actual - expected) > tol:
        raise AssertionError(f"{what or 'value'}: expected ~{expected}, got {actual}")


def truthy(value, what=""):
    if not value:
        raise AssertionError(f"{what or 'value'} is falsy: {value!r}")


# --------------------------------------------------------------- unit tests


@test("geometry: POINT Z, BOX3D, POLYGON, CRS prefix, 2D point")
def t_geometry():
    g = geometry.parse("POINT Z(1 2 3)")
    eq(g.kind, geometry.POINT)
    eq(g.coords[0], (1.0, 2.0, 3.0))

    g = geometry.parse("BOX3D(-1 -2 -3, 4 5 6)")
    eq(g.kind, geometry.BOX)
    eq(g.rings[0][0], (-1.0, -2.0, -3.0))
    eq(g.rings[0][1], (4.0, 5.0, 6.0))

    g = geometry.parse("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))")
    eq(g.kind, geometry.AREA)
    eq(len(g.rings[0]), 5)

    g = geometry.parse("<http://www.opengis.net/def/crs/OGC/1.3/CRS84> POINT(7.4 46.9)")
    eq(g.kind, geometry.POINT)
    close(g.coords[0][0], 7.4)

    g = geometry.parse("MULTIPOINT Z(1 1 1, 2 2 2)")
    eq(len(g.coords), 2)

    eq(geometry.parse("just a label"), None)
    eq(geometry.parse(""), None)

    truthy(geometry.looks_geographic([(7.4, 46.9, 0.0)]))
    truthy(not geometry.looks_geographic([(1200.0, 30.0, 0.0)]))


@test("geometry: local coordinates are not mistaken for longitude/latitude")
def t_geographic_evidence():
    # A football pitch is 105 x 68 metres — entirely inside the lon/lat
    # envelope. Projected as degrees it would span half a continent.
    pitch = [(0.0, 0.0, 0.0), (105.0, 68.0, 0.0)]
    truthy(geometry.looks_geographic(pitch), "range check alone accepts it")
    truthy(
        not geometry.is_geographic(pitch),
        "but with no evidence of degrees it stays a local frame",
    )
    truthy(
        not geometry.is_geographic(pitch, lonlat_named=False, from_wkt=False),
        "bare x/y columns are never geographic",
    )
    truthy(geometry.is_geographic(pitch, from_wkt=True), "a WKT literal is evidence")
    truthy(
        geometry.is_geographic(pitch, lonlat_named=True),
        "columns named lon/lat are evidence",
    )
    # rete's own 3D literals declare a local frame, whatever the magnitudes.
    truthy(
        not geometry.is_geographic(
            pitch, from_wkt=True, datatypes=["https://w3id.org/rete/geo3#wktLiteral3D"]
        ),
        "geo3 3D literals are local by definition",
    )
    # Real degrees still project.
    swiss = [(6.14, 46.2, 0.0), (7.44, 46.95, 0.0)]
    truthy(geometry.is_geographic(swiss, from_wkt=True), "actual lon/lat projects")
    placement = geometry.Placement(geographic=True, ref_lon=6.14, ref_lat=46.2, scale=1.0)
    east = placement.apply((7.44, 46.2, 0.0))[0]
    truthy(90_000 < east < 110_000, f"1.3 deg of longitude is ~100 km ({east:.0f} m)")


@test("geometry: placement scales, recentres and converts Y-up")
def t_placement():
    p = geometry.Placement(scale=0.001)
    eq(p.apply((1000.0, 2000.0, 3000.0)), (1.0, 2.0, 3.0))

    p = geometry.Placement(scale=1.0, axis_up="Y")
    eq(p.apply((1.0, 2.0, 3.0)), (1.0, -3.0, 2.0))

    p = geometry.Placement(scale=1.0, flip_x=True)
    eq(p.apply((5.0, 0.0, 0.0)), (-5.0, 0.0, 0.0))

    coords = [(0.0, 0.0, 0.0), (100.0, 0.0, 0.0)]
    plain = geometry.Placement()
    close(geometry.fit_scale(coords, 10.0, plain), 0.1, what="fit scale")
    eq(geometry.centre_of(coords, plain), (50.0, 0.0, 0.0))


@test("detect: roles from values, names and known predicates")
def t_detect():
    C = engine.Cell
    eq(detect.classify_column("mesh", [C("iri", "https://x.org/a.glb")]), detect.ASSET)
    eq(detect.classify_column("g", [C("literal", "POINT Z(1 2 3)")]), detect.GEOMETRY)
    eq(detect.classify_column("pic", [C("iri", "https://x.org/a.jpg")]), detect.IMAGE)
    eq(detect.classify_column("c", [C("literal", "#ff8800")]), detect.COLOR)
    eq(
        detect.classify_column("n", [C("literal", "42", "http://www.w3.org/2001/XMLSchema#integer")]),
        detect.NUMBER,
    )
    eq(
        detect.classify_column("d", [C("literal", "1789-07-14", "http://www.w3.org/2001/XMLSchema#date")]),
        detect.TIME,
    )
    eq(detect.classify_column("s", [C("iri", "https://x.org/thing")]), detect.ENTITY)
    eq(detect.classify_column("label", [C("literal", "Femur")]), detect.LABEL)
    # A column named "model" holding geometry is geometry, not an asset.
    eq(detect.classify_column("model", [C("literal", "POINT Z(0 0 0)")]), detect.GEOMETRY)
    # A known predicate wins outright.
    eq(
        detect.classify_column(
            "x", [C("literal", "whatever")], predicate="https://w3id.org/rete/anatomy#glbFile"
        ),
        detect.ASSET,
    )
    # IIIF URLs have no extension.
    truthy(detect.is_image_url("https://iiif.example.org/img/abc/full/full/0/default.jpg"))

    # CAD / BIM asset URLs are recognised, whatever the column is called.
    eq(detect.classify_column("m", [C("iri", "https://x.org/house.ifc")]), detect.ASSET)
    eq(detect.classify_column("m", [C("iri", "https://x.org/plan.dxf")]), detect.ASSET)
    truthy(detect.is_model_url("https://data.graphplaza.com/cad/fzk-haus.glb"))
    truthy(detect.url_extension("https://x.org/a.ifc") == ".ifc")
    # The cad: vocabulary is pinned for determinism.
    eq(
        detect.classify_column(
            "glb", [C("iri", "https://x.org/x.glb")], predicate="https://w3id.org/rete/cad#glbModel"
        ),
        detect.ASSET,
    )
    eq(
        detect.classify_column(
            "cls", [C("literal", "IfcWallStandardCase")], predicate="https://w3id.org/rete/cad#ifcClass"
        ),
        detect.CLASS,
    )
    eq(
        detect.classify_column(
            "e", [C("literal", "2.7", "http://www.w3.org/2001/XMLSchema#decimal")],
            predicate="https://w3id.org/rete/cad#elevation",
        ),
        detect.NUMBER,
    )
    eq(
        detect.classify_column(
            "m", [C("iri", "https://x.org/whatever")], predicate="https://w3id.org/rete/cad#ifcModel"
        ),
        detect.ASSET,
    )

    # Decimal seconds named like a time are a time, not a measurement — this is
    # how the subtitles, dance and tracking graphs publish their timelines.
    seconds = [C("literal", "4.000", "http://www.w3.org/2001/XMLSchema#decimal")]
    eq(detect.classify_column("start", seconds), detect.TIME)
    eq(detect.classify_column("endTime", seconds), detect.TIME_END)
    eq(detect.classify_column("t", seconds), detect.TIME)
    # …but a number named like a measurement stays a number.
    eq(detect.classify_column("mass", seconds), detect.NUMBER)
    eq(
        detect.classify_column("x", seconds, predicate="https://w3id.org/rete/tracking#t"),
        detect.TIME,
    )


@test("detect: the query text says which predicate bound each variable")
def t_query_predicates():
    found = detect.predicates_from_query(
        """PREFIX geo3: <https://w3id.org/rete/geo3#>
           PREFIX t: <https://w3id.org/rete/tracking#>
           SELECT ?s ?wkt ?t ?kind WHERE {
             ?s geo3:asWKT3D ?wkt ; t:t ?t ; a ?kind .
             ?s <https://w3id.org/rete/anatomy#glbFile> ?mesh .
           }"""
    )
    eq(found["wkt"], "https://w3id.org/rete/geo3#asWKT3D")
    eq(found["t"], "https://w3id.org/rete/tracking#t")
    eq(found["mesh"], "https://w3id.org/rete/anatomy#glbFile")
    eq(found["kind"], detect.RDF_TYPE)
    eq(detect.predicates_from_query(""), {})


@test("detect: coordinate columns are recognised by axis name")
def t_axes():
    C = engine.Cell
    num = "http://www.w3.org/2001/XMLSchema#decimal"

    class FakeResult:
        vars = ["obj", "x", "y", "t"]

        def column(self, var):
            return {
                "obj": [C("iri", "https://x.org/p1")],
                "x": [C("literal", "12.5", num)],
                "y": [C("literal", "30.0", num)],
                "t": [C("literal", "0.2", num)],
            }[var]

    result = FakeResult()
    roles = detect.classify_result(result)
    eq(roles["x"], detect.NUMBER)
    eq(roles["t"], detect.TIME, "t is time, not a number")
    binding = detect.resolve(result, roles, {})
    eq(binding.xyz, ["x", "y", ""], "x/y recognised as axes")

    # One lone axis column is a measurement, not a position.
    class OneAxis(FakeResult):
        vars = ["obj", "x"]

    eq(detect.resolve(OneAxis(), detect.classify_result(OneAxis()), {}).xyz, [])


@test("timeline: dates, BCE years, durations, clock times")
def t_timeline():
    epoch = timeline.to_seconds("1970-01-01")
    close(epoch, 0.0, 1.0, what="epoch")
    later = timeline.to_seconds("1970-01-02")
    close(later - epoch, 86400.0, 1.0, what="one day")
    truthy(timeline.to_seconds("-0500-01-01") < timeline.to_seconds("1000-01-01"))
    close(timeline.to_seconds("PT1M30S", "http://www.w3.org/2001/XMLSchema#duration"), 90.0)
    close(timeline.to_seconds("00:01:30", "http://www.w3.org/2001/XMLSchema#time"), 90.0)
    close(timeline.to_seconds("12.5"), 12.5)
    eq(timeline.to_seconds(""), None)

    # Typed numbers are numbers, not years. Decimal seconds are how the
    # subtitles, dance and tracking graphs publish their timelines, and reading
    # "0.5" as the year 0 collapses a whole trajectory onto two frames.
    decimal = "http://www.w3.org/2001/XMLSchema#decimal"
    close(timeline.to_seconds("0.5", decimal), 0.5)
    close(timeline.to_seconds("1.0", decimal), 1.0)
    close(timeline.to_seconds("42", "http://www.w3.org/2001/XMLSchema#integer"), 42.0)
    truthy(
        timeline.to_seconds("1.5", decimal) < timeline.to_seconds("2.0", decimal),
        "decimal seconds keep their order",
    )
    # Real dates still parse, typed or not.
    truthy(
        timeline.to_seconds("2022-11-20", "http://www.w3.org/2001/XMLSchema#date") > 1.6e9,
        "a typed date is seconds since the epoch",
    )
    year1982 = timeline.to_seconds("1982", "http://www.w3.org/2001/XMLSchema#gYear")
    truthy(3.7e8 < year1982 < 4.1e8, f"gYear 1982 lands in 1982 ({year1982})")
    truthy(timeline.to_seconds("1789-07-14T12:00:00") < 0, "an untyped ISO date parses")

    mapper = timeline.Mapper([0.0, 100.0], 1, 101)
    close(mapper.frame(0.0), 1.0)
    close(mapper.frame(100.0), 101.0)
    close(mapper.frame(50.0), 51.0)


@test("materials: hex, rgb() and names parse; ramps and keys are stable")
def t_materials():
    white = materials.parse_color("#ffffff")
    close(white[0], 1.0)
    black = materials.parse_color("#000")
    close(black[0], 0.0)
    truthy(materials.parse_color("rgb(255, 0, 0)") is not None)
    truthy(materials.parse_color("teal") is not None)
    eq(materials.parse_color("not a colour"), None)

    eq(materials.color_for_key("x"), materials.color_for_key("x"))
    truthy(materials.color_for_key("a") != materials.color_for_key("b"))
    low, high = materials.ramp(0.0), materials.ramp(1.0)
    truthy(low != high, "ramp ends differ")


@test("props: statements become drivable custom properties and round-trip")
def t_props():
    C = engine.Cell
    obj = bpy.data.objects.new("prop-test", None)
    rprops.stamp_identity(obj, "https://x.org/thing", "test.rete")
    written = rprops.stamp(
        obj,
        [
            ("https://x.org/vocab#mass", C("literal", "12.5", "http://www.w3.org/2001/XMLSchema#double")),
            ("https://x.org/vocab#name", C("literal", "Thing")),
            ("https://x.org/vocab#tag", C("literal", "a")),
            ("https://x.org/vocab#tag", C("literal", "b")),
            ("http://www.w3.org/1999/02/22-rdf-syntax-ns#type", C("iri", "https://x.org/vocab#Widget")),
        ],
    )
    truthy(written >= 3, "properties written")
    eq(obj["mass"], 12.5)
    eq(obj["name"], "Thing")
    eq(rprops.values_of(obj, "tag"), ["a", "b"])
    eq(rprops.classes_of(obj), ["https://x.org/vocab#Widget"])
    eq(rprops.predicate_map(obj)["mass"], "https://x.org/vocab#mass")
    eq(rprops.iri_of(obj), "https://x.org/thing")
    close(rprops.number_of(obj, "mass"), 12.5)

    # Numeric custom properties must be drivable — the whole point of them.
    driver = obj.driver_add("location", 0).driver
    var = driver.variables.new()
    var.type = "SINGLE_PROP"
    var.targets[0].id = obj
    var.targets[0].data_path = '["mass"]'
    driver.expression = "var * 0.1"
    bpy.context.view_layer.update()
    truthy(obj.animation_data is not None, "driver attached")


# ------------------------------------------------------- fixture and E2E


FIXTURE_NT_TEMPLATE = """\
<https://x.org/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://x.org/Bone> .
<https://x.org/a> <http://www.w3.org/2000/01/rdf-schema#label> "Alpha" .
<https://x.org/a> <https://w3id.org/rete/geo3#asWKT3D> "POINT Z(0 0 0)" .
<https://x.org/a> <https://w3id.org/rete/geo3#box> "BOX3D(-10 -10 -10, 10 10 10)" .
<https://x.org/a> <https://x.org/vocab#mass> "2.5"^^<http://www.w3.org/2001/XMLSchema#double> .
<https://x.org/a> <https://x.org/vocab#colour> "#ff0000" .
<https://x.org/a> <http://purl.org/dc/terms/date> "1900-01-01"^^<http://www.w3.org/2001/XMLSchema#date> .
<https://x.org/a> <https://x.org/vocab#note> "the first one" .
<https://x.org/b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://x.org/Bone> .
<https://x.org/b> <http://www.w3.org/2000/01/rdf-schema#label> "Beta" .
<https://x.org/b> <https://w3id.org/rete/geo3#asWKT3D> "POINT Z(1000 0 0)" .
<https://x.org/b> <https://w3id.org/rete/geo3#box> "BOX3D(990 -10 -10, 1010 10 10)" .
<https://x.org/b> <https://x.org/vocab#mass> "7.5"^^<http://www.w3.org/2001/XMLSchema#double> .
<https://x.org/b> <https://x.org/vocab#colour> "#00ff00" .
<https://x.org/b> <http://purl.org/dc/terms/date> "1950-01-01"^^<http://www.w3.org/2001/XMLSchema#date> .
<https://x.org/b> <https://x.org/vocab#partOf> <https://x.org/a> .
<https://x.org/b> <https://x.org/vocab#touches> <https://x.org/a> .
<https://x.org/c> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://x.org/Muscle> .
<https://x.org/c> <http://www.w3.org/2000/01/rdf-schema#label> "Gamma" .
<https://x.org/c> <https://w3id.org/rete/geo3#asWKT3D> "POINT Z(0 1000 500)" .
<https://x.org/c> <https://x.org/vocab#mass> "1.0"^^<http://www.w3.org/2001/XMLSchema#double> .
<https://x.org/c> <http://purl.org/dc/terms/date> "2000-01-01"^^<http://www.w3.org/2001/XMLSchema#date> .
<https://x.org/c> <https://x.org/vocab#partOf> <https://x.org/a> .
<https://x.org/c> <https://x.org/vocab#touches> <https://x.org/b> .
<https://x.org/c> <https://x.org/vocab#glb> "{glb}" .
<https://x.org/c> <https://x.org/vocab#meshNode> "rete-test-cube" .
"""

FIXTURE_QUERY = """\
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX geo3: <https://w3id.org/rete/geo3#>
PREFIX v: <https://x.org/vocab#>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?s ?label ?wkt ?box ?mass ?when WHERE {
  ?s rdfs:label ?label ; geo3:asWKT3D ?wkt ; v:mass ?mass ; dct:date ?when .
  OPTIONAL { ?s geo3:box ?box }
} ORDER BY ?label
"""

STATE = {}


def make_glb(directory: str) -> str:
    """Export a named cube as a .glb so the asset path can be tested offline."""
    mesh = bpy.data.meshes.new("rete-test-cube-mesh")
    import bmesh

    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    bm.to_mesh(mesh)
    bm.free()
    obj = bpy.data.objects.new("rete-test-cube", mesh)
    bpy.context.scene.collection.objects.link(obj)

    for other in bpy.context.selected_objects:
        other.select_set(False)
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj

    path = os.path.join(directory, "cube.glb")
    bpy.ops.export_scene.gltf(filepath=path, export_format="GLB", use_selection=True)
    bpy.data.objects.remove(obj, do_unlink=True)
    return path


@test("fixture: a .rete is built and queried in memory")
def t_fixture():
    truthy(engine.available(), "engine importable")
    directory = tempfile.mkdtemp(prefix="rete-blender-test-")
    STATE["dir"] = directory
    glb = make_glb(directory)
    STATE["glb"] = glb

    nt = FIXTURE_NT_TEMPLATE.format(glb=glb.replace("\\", "/"))
    mod = engine.engine()
    path = os.path.join(directory, "fixture.rete")
    mod.Builder().add(nt, "nt").card(title="Fixture", license="CC0-1.0").export(path)
    STATE["source"] = path

    result = engine.select(path, FIXTURE_QUERY)
    eq(len(result), 3, "fixture rows")
    eq(result.vars, ["s", "label", "wkt", "box", "mass", "when"])
    STATE["result"] = result


@test("build: objects, placement, materials and inherited properties")
def t_build():
    settings = builder.Settings(
        source=STATE["source"],
        query=FIXTURE_QUERY,
        collection_name="test-build",
        scale_mode="MM",
        recentre=False,
        deep_properties=True,
        material_mode="AUTO",
        point_style="SPHERE",
        point_size=0.05,
        layout="GEOMETRY",
    )
    report = builder.build(STATE["result"], settings)
    eq(report.objects, 3, "objects built")
    truthy(report.properties > 0, "properties inherited")

    by_iri = rprops.objects_with_iri()
    truthy("https://x.org/a" in by_iri, "entity a in scene")
    alpha = by_iri["https://x.org/a"]
    beta = by_iri["https://x.org/b"]

    # POINT Z(1000 0 0) in millimetres is one metre along X.
    close(beta.location[0], 1.0, 1e-3, what="beta X")
    close(alpha.location[0], 0.0, 1e-3, what="alpha X")

    # The box sized the marker: a 20 mm box is 0.02 m across. `dimensions` only
    # reflects a new scale after the depsgraph runs, hence the explicit update.
    bpy.context.view_layer.update()
    close(alpha.dimensions[0], 0.02, 1e-3, what="alpha width from BOX3D")
    close(alpha.dimensions[2], 0.02, 1e-3, what="alpha height from BOX3D")

    # Deep properties arrived, with their predicate map.
    eq(alpha["note"], "the first one")
    close(rprops.number_of(alpha, "mass"), 2.5)
    eq(rprops.classes_of(alpha), ["https://x.org/Bone"])
    eq(rprops.predicate_map(alpha)["note"], "https://x.org/vocab#note")
    truthy(alpha.material_slots and alpha.material_slots[0].material is not None, "material assigned")
    STATE["by_iri"] = by_iri


@test("build: an asset is imported and a named node isolated")
def t_asset():
    query = """\
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX v: <https://x.org/vocab#>
SELECT ?s ?label ?glb ?meshNode WHERE {
  ?s rdfs:label ?label ; v:glb ?glb ; v:meshNode ?meshNode .
}
"""
    result = engine.select(STATE["source"], query)
    eq(len(result), 1, "asset rows")
    roles = detect.classify_result(result)
    eq(roles["glb"], detect.ASSET, "glb column detected as an asset")
    eq(roles["meshNode"], detect.MESH_NODE, "meshNode column detected")

    settings = builder.Settings(
        source=STATE["source"],
        collection_name="test-asset",
        deep_properties=False,
        material_mode="NONE",
        layout="GEOMETRY",
    )
    report = builder.build(result, settings)
    eq(report.assets, 1, "assets imported")
    coll = bpy.data.collections["test-asset"]
    gamma = next(o for o in coll.objects if o.get(rprops.IRI) == "https://x.org/c")
    eq(gamma.type, "MESH", "isolated node is a mesh")
    # glTF splits shared vertices by normal and stores triangles only, so a
    # cube arrives as 24 vertices and 12 faces rather than 8 and 6.
    truthy(len(gamma.data.vertices) >= 8, "the isolated node carries the cube's geometry")
    truthy(len(gamma.data.polygons) >= 6, "the isolated node has the cube's faces")


@test("relations: a predicate becomes parenting, and becomes edge geometry")
def t_relations():
    by_iri = STATE["by_iri"]
    coll = bpy.data.collections["test-build"]
    made, _ = relations.apply(
        "PARENT", STATE["source"], by_iri, "https://x.org/vocab#partOf", coll
    )
    eq(made, 2, "parented rows")
    eq(by_iri["https://x.org/b"].parent, by_iri["https://x.org/a"])

    made, _ = relations.apply(
        "EDGES", STATE["source"], by_iri, "https://x.org/vocab#touches", coll
    )
    eq(made, 2, "edges drawn")
    edge_obj = bpy.data.objects.get("edges:touches")
    truthy(edge_obj is not None and len(edge_obj.data.edges) == 2, "edge mesh has both edges")


@test("time: dates become keyframes over the scene's frame range")
def t_time():
    settings = builder.Settings(
        source=STATE["source"],
        collection_name="test-time",
        scale_mode="MM",
        deep_properties=False,
        material_mode="NONE",
        time_mode="APPEAR",
        frame_start=1,
        frame_end=100,
        layout="GEOMETRY",
    )
    report = builder.build(STATE["result"], settings)
    truthy(report.keyframed >= 2, "objects keyframed")
    eq(bpy.context.scene.frame_start, 1)
    eq(bpy.context.scene.frame_end, 100)

    coll = bpy.data.collections["test-time"]
    animated = [o for o in coll.objects if o.animation_data and o.animation_data.action]
    truthy(animated, "at least one object animated")
    curves = list(timeline.iter_fcurves(animated[0].animation_data.action))
    truthy(any(c.data_path in ("hide_viewport", "hide_render") for c in curves), "visibility keyed")

    # The earliest row must be visible from the start, the latest must not.
    scene = bpy.context.scene
    by_iri = {o.get(rprops.IRI): o for o in coll.objects if o.get(rprops.IRI)}
    scene.frame_set(1)
    truthy(not by_iri["https://x.org/a"].hide_render, "1900 row visible at frame 1")
    truthy(by_iri["https://x.org/c"].hide_render, "2000 row hidden at frame 1")
    scene.frame_set(100)
    truthy(not by_iri["https://x.org/c"].hide_render, "2000 row visible at the end")
    scene.frame_set(1)


@test("time: a trajectory becomes one moving object per tracked thing")
def t_motion_path():
    # Shaped like the tracking graph: one sample node per (object, instant),
    # the moving thing named in a second column, positions as plain x/y.
    lines = []
    for obj_id in ("p1", "p2"):
        for step in range(4):
            sample = f"https://x.org/pos/{obj_id}-{step}"
            lines.append(f'<{sample}> <https://x.org/t#object> <https://x.org/obj/{obj_id}> .')
            lines.append(
                f'<{sample}> <https://x.org/t#t> "{step * 0.5}"'
                '^^<http://www.w3.org/2001/XMLSchema#decimal> .'
            )
            lines.append(
                f'<{sample}> <https://x.org/t#x> "{step * 2.0}"'
                '^^<http://www.w3.org/2001/XMLSchema#decimal> .'
            )
            lines.append(
                f'<{sample}> <https://x.org/t#y> "{step * (1.0 if obj_id == "p1" else -1.0)}"'
                '^^<http://www.w3.org/2001/XMLSchema#decimal> .'
            )
    path = os.path.join(STATE["dir"], "track.rete")
    engine.engine().Builder().add("\n".join(lines) + "\n", "nt").export(path)

    result = engine.select(
        path,
        """PREFIX t: <https://x.org/t#>
           SELECT ?sample ?object ?t ?x ?y WHERE {
             ?sample t:object ?object ; t:t ?t ; t:x ?x ; t:y ?y .
           } ORDER BY ?t""",
    )
    eq(len(result), 8, "trajectory samples")

    roles = detect.classify_result(result)
    binding = detect.resolve(result, roles, {})
    eq(binding.xyz, ["x", "y", ""], "positions read from x/y columns")
    eq(builder._path_group_var(result.rows, binding), "object", "grouped by the moving object")

    settings = builder.Settings(
        source=path,
        collection_name="test-path",
        time_mode="PATH",
        frame_start=1,
        frame_end=48,
        scale_mode="M",
        recentre=False,
        deep_properties=False,
        material_mode="NONE",
        layout="GEOMETRY",
    )
    report = builder.build(result, settings)
    eq(report.objects, 2, "one object per tracked thing, not one per sample")
    eq(report.keyframed, 2, "both objects keyframed")

    coll = bpy.data.collections["test-path"]
    mover = next(o for o in coll.objects if o.get(rprops.IRI) == "https://x.org/obj/p1")
    curves = list(timeline.iter_fcurves(mover.animation_data.action))
    truthy(any(c.data_path == "location" for c in curves), "location is animated")

    scene = bpy.context.scene
    scene.frame_set(1)
    start_x = mover.matrix_world.translation[0]
    scene.frame_set(48)
    end_x = mover.matrix_world.translation[0]
    truthy(end_x > start_x, f"the object actually moves ({start_x:.3f} -> {end_x:.3f})")
    # x runs 0..6 in the fixture; if these metres were read as degrees of
    # longitude the object would travel hundreds of kilometres instead.
    travelled = end_x - start_x
    close(travelled, 6.0, 0.1, what="distance travelled, in metres")
    scene.frame_set(1)


@test("CAD: an IFC building graph builds — geometry in metres, IFC class, storeys")
def t_cad_graph():
    src = os.path.join(REPO, "web", "fzk-haus.rete")
    if not os.path.exists(src):
        print("       (web/fzk-haus.rete absent — skipping)")
        return

    query = """
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX geo3: <https://w3id.org/rete/geo3#>
PREFIX cad:  <https://w3id.org/rete/cad#>
SELECT ?s ?ifc ?wkt ?box WHERE {
  ?s cad:ifcClass ?ifc ; geo:hasGeometry ?g .
  ?g geo3:asWKT3D ?wkt ; geo3:box ?box .
} LIMIT 80
"""
    result = engine.select(src, query)
    truthy(len(result) > 20, f"elements returned ({len(result)})")
    roles = detect.classify_result(result)
    eq(roles["wkt"], detect.GEOMETRY, "geo3:asWKT3D is geometry")
    eq(roles["box"], detect.GEOMETRY, "geo3:box is geometry")
    eq(roles["ifc"], detect.CLASS, "cad:ifcClass is a class")

    name = fresh_local("cad-build")
    report = builder.build(
        result,
        builder.Settings(
            source=src,
            query=query,
            collection_name=name,
            scale_mode="M",        # IFC coordinates are metres
            recentre=True,
            point_style="CUBE",    # box geometry sizes the massing
            material_mode="CLASS", # colour by IFC class
            deep_properties=True,
            import_assets=False,
            layout="GEOMETRY",
        ),
    )
    truthy(report.objects > 20, f"objects built ({report.objects})")
    objects = list(bpy.data.collections[name].all_objects)
    bpy.context.view_layer.update()

    # FZK-Haus is a two-storey house ~12 x 10 x 7 m; sized boxes must be
    # building-scale, not millimetre dots or kilometre slabs.
    sized = [o for o in objects if max(o.dimensions) > 1e-3]
    truthy(sized, "boxes sized the elements")
    biggest = max(max(o.dimensions) for o in sized)
    truthy(1.0 < biggest < 40.0, f"largest element is building-scale ({biggest:.1f} m)")

    # The IFC class rode in as an inherited property, and coloured the scene.
    classed = [o for o in objects if str(o.get("ifcClass", "")).startswith("Ifc")]
    truthy(classed, "elements carry their IFC class as an inherited property")
    distinct_classes = {str(o.get("ifcClass", "")) for o in classed}
    truthy(len(distinct_classes) >= 2, f"several IFC classes present ({len(distinct_classes)})")
    truthy(
        any(o.material_slots and o.material_slots[0].material for o in objects),
        "elements coloured by class",
    )
    STATE["cad_src"] = src


@test("CAD: BOT topology becomes collections, adjacency becomes constraints")
def t_cad_relations():
    src = STATE.get("cad_src")
    if not src:
        print("       (no CAD source — skipping)")
        return
    # Storeys and their elements: bot:containsElement is the containment edge.
    query = """
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX cad:  <https://w3id.org/rete/cad#>
SELECT ?s ?ifc ?storey WHERE {
  ?s cad:ifcClass ?ifc ; cad:inStorey ?storey .
} LIMIT 120
"""
    result = engine.select(src, query)
    truthy(len(result) > 10, f"elements in storeys ({len(result)})")

    name = fresh_local("cad-topo")
    report = builder.build(
        result,
        builder.Settings(
            source=src,
            query=query,
            collection_name=name,
            scale_mode="M",
            deep_properties=False,
            material_mode="CLASS",
            import_assets=False,
            layout="GRID",
            relation_mode="COLLECTION",
            relation_predicate="https://w3id.org/rete/cad#inStorey",
        ),
    )
    truthy(report.objects > 10, "elements built")
    truthy(report.relations > 0, f"storey grouping applied ({report.relations})")
    storeys = [c for c in bpy.data.collections if c.name.startswith("Storey") or "storey" in c.name.lower()]
    # Grouping created named sub-collections (the storeys), whatever they're called.
    truthy(len(bpy.data.collections[name].children) >= 1, "storey collections created")

    # Now the "building topology as physics" path: build the spaces and turn
    # cad:adjacentSpace into rigid-body constraints between them.
    space_q = """
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX geo3: <https://w3id.org/rete/geo3#>
PREFIX cad:  <https://w3id.org/rete/cad#>
SELECT ?s ?wkt WHERE {
  ?s cad:ifcClass "IfcSpace" ; geo:hasGeometry ?g .
  ?g geo3:asWKT3D ?wkt .
}
"""
    spaces = engine.select(src, space_q)
    truthy(len(spaces) >= 3, f"spaces returned ({len(spaces)})")
    sname = fresh_local("cad-spaces")
    builder.build(
        spaces,
        builder.Settings(
            source=src, collection_name=sname, scale_mode="M", recentre=True,
            deep_properties=False, material_mode="NONE", import_assets=False,
            layout="GEOMETRY", point_style="SPHERE", point_size=0.5,
        ),
    )
    by_iri = {o.get(rprops.IRI): o for o in bpy.data.collections[sname].all_objects if o.get(rprops.IRI)}
    edges = [
        (by_iri.get(s), by_iri.get(o))
        for s, o in _pairs(src, list(by_iri), "https://w3id.org/rete/cad#adjacentSpace")
        if by_iri.get(s) and by_iri.get(o)
    ]
    made = physics.constraint_network(edges, constraint_type="FIXED")
    truthy(made > 0, f"space adjacency became rigid-body constraints ({made})")
    print(f"       {report.summary()}, {len(spaces)} spaces, {made} adjacency constraints")


@test("IFC: a raw .ifc URL imports as meshes (or degrades with a clear message)")
def t_ifc_import():
    ifc = os.path.join(REPO, "data", "cad", "raw", "FZK-Haus.ifc")
    if not os.path.exists(ifc):
        print("       (FZK-Haus.ifc absent — skipping)")
        return
    url = "file://" + ifc.replace("\\", "/")

    try:
        import ifcopenshell  # noqa: F401

        have_ifc = True
    except Exception:
        have_ifc = False

    if not have_ifc:
        # The graceful path: a clear, actionable message, no crash.
        try:
            assets.import_asset(url)
            raise AssertionError("expected an IOError without ifcopenshell")
        except IOError as exc:
            truthy("ifcopenshell" in str(exc), "message names ifcopenshell")
        print("       ifcopenshell absent — graceful message verified")
        return

    objects = assets.import_asset(url)
    truthy(len(objects) > 20, f"IFC elements tessellated ({len(objects)})")
    meshes = [o for o in objects if o.type == "MESH" and o.data.vertices]
    truthy(meshes, "elements have real geometry")
    truthy(sum(len(o.data.vertices) for o in meshes) > 1000, "substantial geometry")
    # BIM identity survived onto the imported meshes.
    classed = [o for o in objects if o.get("ifcClass")]
    truthy(classed, "imported elements carry their IFC class")
    walls = [o for o in objects if "Wall" in str(o.get("ifcClass", ""))]
    truthy(walls, "the house has walls")

    # World coordinates: FZK-Haus is a ~12 m house, so the whole model spans a
    # sane building extent rather than collapsing to a point.
    xs = [v.co.x for o in meshes for v in o.data.vertices]
    span = max(xs) - min(xs)
    truthy(2.0 < span < 200.0, f"the model is building-scale ({span:.1f} m across)")

    # And it drops into a scene through the normal build path: a graph whose one
    # row points a cad:ifcModel column straight at the .ifc URL.
    directory = STATE.get("dir") or tempfile.mkdtemp(prefix="rete-ifc-")
    STATE["dir"] = directory
    graph_path = os.path.join(directory, "ifc-ref.rete")
    engine.engine().Builder().add(
        f'<https://x.org/b> <https://w3id.org/rete/cad#ifcModel> <{url}> .\n'
        '<https://x.org/b> <http://www.w3.org/2000/01/rdf-schema#label> "FZK-Haus" .\n',
        "nt",
    ).export(graph_path)
    result = engine.select(
        graph_path,
        "PREFIX cad: <https://w3id.org/rete/cad#>\n"
        "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n"
        "SELECT ?s ?label ?m WHERE { ?s cad:ifcModel ?m ; rdfs:label ?label }",
    )
    eq(len(result), 1, "the ifcModel row")
    eq(detect.classify_result(result)["m"], detect.ASSET, "cad:ifcModel is an asset")
    name = fresh_local("ifc-scene")
    report = builder.build(
        result,
        builder.Settings(
            source=graph_path,
            collection_name=name,
            import_assets=True,
            max_assets=1,
            deep_properties=False,
            material_mode="CLASS",
            layout="GEOMETRY",
        ),
    )
    eq(report.assets, 1, "the IFC imported through the build path")
    scene_meshes = [o for o in bpy.data.collections[name].all_objects if o.type == "MESH"]
    truthy(scene_meshes, "IFC elements are in the scene")
    print(f"       imported {len(objects)} IFC elements, {span:.1f} m across, "
          f"{len(scene_meshes)} placed in the scene")


@test("physics: rigid bodies, mass from a property, relation constraints")
def t_physics():
    by_iri = STATE["by_iri"]
    objects = list(by_iri.values())
    count = physics.add_bodies(objects, body_type="ACTIVE", shape="CONVEX_HULL")
    eq(count, 3, "bodies added")
    truthy(all(o.rigid_body is not None for o in objects), "every object has a body")

    scaled = physics.scale_masses(objects, "mass", low=1.0, high=10.0)
    eq(scaled, 3, "masses scaled")
    heaviest = by_iri["https://x.org/b"]   # mass 7.5 is the largest in the fixture
    lightest = by_iri["https://x.org/c"]   # mass 1.0 is the smallest
    truthy(heaviest.rigid_body.mass > lightest.rigid_body.mass, "mass ordering preserved")

    edges = relations.fetch_edges(STATE["source"], by_iri, "https://x.org/vocab#touches")
    made = physics.constraint_network(
        [(a, b) for a, b, _ in edges if b is not None], constraint_type="FIXED"
    )
    eq(made, 2, "constraints created")
    holder = bpy.data.collections.get(physics.CONSTRAINT_COLLECTION)
    truthy(holder and len(holder.objects) == 2, "constraint empties exist")
    empty = holder.objects[0]
    truthy(empty.rigid_body_constraint is not None, "constraint settings present")
    eq(empty.rigid_body_constraint.type, "FIXED")


@test("point cloud: rows become one mesh with named attributes")
def t_point_cloud():
    settings = builder.Settings(
        source=STATE["source"],
        collection_name="test-points",
        scale_mode="MM",
        point_cloud=True,
        deep_properties=False,
        layout="GEOMETRY",
    )
    report = builder.build(STATE["result"], settings)
    eq(report.objects, 1, "one point-cloud object")
    obj = bpy.data.collections["test-points"].objects[0]
    eq(len(obj.data.vertices), 3, "one vertex per row")
    truthy("mass" in obj.data.attributes, "numeric column became an attribute")
    eq(obj.data.attributes["mass"].data_type, "FLOAT")
    truthy(obj.modifiers and obj.modifiers[0].type == "NODES", "geometry nodes modifier attached")
    table = attributes.category_table(obj)
    truthy("label" in table and len(table["label"]) == 3, "categorical table written")


@test("export: the scene becomes a .rete that answers queries about itself")
def t_export():
    objects = [o for o in bpy.data.collections["test-build"].all_objects]
    truthy(objects, "objects to export")
    nt, count = export.scene_to_ntriples(
        objects, base="https://example.org/s/", scene_name="TestScene", keep_iris=True
    )
    eq(count, len(objects))
    truthy("https://w3id.org/rete/scene#locationX" in nt, "transforms emitted")
    truthy("https://x.org/vocab#note" in nt, "inherited predicate restored on export")

    path = os.path.join(STATE["dir"], "scene.rete")
    stats = export.build_rete(nt, path, title="Test scene")
    truthy(int(stats.get("statements", 0)) > 20, "statements built")

    rows = engine.select(
        path,
        """PREFIX scene: <https://w3id.org/rete/scene#>
           PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
           SELECT ?label ?x WHERE { ?o a scene:Object ; rdfs:label ?label ; scene:locationX ?x }
           ORDER BY ?label""",
    )
    truthy(len(rows) >= 3, "exported objects queryable")
    labels = [r["label"].value for r in rows.rows]
    truthy("Alpha" in labels, "Alpha survived the round trip")

    # The original entity IRI is preserved, not re-minted.
    same = engine.select(
        path, "SELECT ?p ?o WHERE { <https://x.org/a> ?p ?o }"
    )
    truthy(len(same) > 3, "the original IRI is still the subject")


@test("drivers: rete() is registered and answers from the fixture")
def t_drivers():
    from addon import drivers

    drivers.register()
    drivers.set_default_source(STATE["source"])
    truthy("rete" in bpy.app.driver_namespace, "driver function registered")
    value = drivers.rete_count("?s a <https://x.org/Bone>")
    eq(value, 2.0, "count through the driver namespace")
    eq(drivers.rete_value("SELECT ?x WHERE { }", default=3.0), 3.0, "failure returns the default")


@test("add-on: registers and unregisters cleanly")
def t_register():
    addon.register()
    truthy(hasattr(bpy.context.scene, "rete"), "scene settings registered")
    settings = bpy.context.scene.rete
    settings.source = STATE["source"]
    eq(settings.to_build_settings().source, STATE["source"])
    truthy(hasattr(bpy.types, "RETE_PT_graph"), "panel registered")
    addon.unregister()
    truthy(not hasattr(bpy.context.scene, "rete"), "scene settings removed")
    addon.register()  # leave it registered, as a normal session would


def main() -> None:
    print("\nrete Blender add-on — headless tests")
    print(f"Blender {bpy.app.version_string}, engine {engine.version() or 'MISSING'}\n")

    for fn in (
        t_geometry, t_geographic_evidence, t_placement, t_detect, t_query_predicates, t_axes,
        t_timeline, t_materials, t_props,
        t_fixture, t_build, t_asset, t_relations, t_time, t_motion_path, t_physics,
        t_cad_graph, t_cad_relations, t_ifc_import,
        t_point_cloud, t_export, t_drivers, t_register,
    ):
        fn()

    print(f"\n{len(PASSED)} passed, {len(FAILED)} failed")
    if FAILED:
        for name, exc in FAILED:
            print(f"  FAILED {name}: {exc}")
        sys.exit(1)
    print("ALL TESTS PASSED")


if __name__ == "__main__":
    main()
