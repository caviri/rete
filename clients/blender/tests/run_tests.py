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
    media,
    physics,
    props as rprops,
    relations,
    splats,
    tiles,
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

    # Maps, videos and CAD/BIM asset URLs are recognised, whatever the column
    # is called — value evidence outranks the name.
    eq(detect.classify_column("m", [C("iri", "https://x.org/world.pmtiles")]), detect.MAP)
    eq(detect.classify_column("v", [C("iri", "https://x.org/clip.mp4")]), detect.VIDEO)
    eq(detect.classify_column("v", [C("iri", "https://x.org/clip.webm")]), detect.VIDEO)
    truthy(detect.is_map_url("https://x.org/a.pmtiles"))
    truthy(detect.is_video_url("https://x.org/a.mov"))
    # A .pmtiles column is not swallowed as the fallback entity.
    class MapResult:
        vars = ["map"]
        query = ""
        def column(self, v):
            return [C("iri", "https://x.org/basemap.pmtiles")]
    b = detect.resolve(MapResult(), detect.classify_result(MapResult()), {})
    eq(b.maps, ["map"], "map column kept as MAP, not entity")
    truthy(b.entity is None, "no fallback entity stole the map column")

    # Gaussian-splat URLs: .splat/.ksplat by extension; .ply stays ASSET (it is
    # sniffed at import); a splat-named column of non-URLs falls back to SPLAT.
    eq(detect.classify_column("m", [C("iri", "https://x.org/scan.splat")]), detect.SPLAT)
    eq(detect.classify_column("m", [C("iri", "https://x.org/scan.ksplat")]), detect.SPLAT)
    eq(detect.classify_column("m", [C("iri", "https://x.org/scan.ply")]), detect.ASSET)
    eq(detect.classify_column("gaussian", [C("literal", "ref-42")]), detect.SPLAT)
    eq(
        detect.classify_column(
            "s", [C("iri", "https://x.org/x")], predicate="https://w3id.org/rete/media#splat"
        ),
        detect.SPLAT,
    )

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


def _varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def _make_raster_pmtiles(tile_png: bytes, path: str) -> str:
    """Synthesize a minimal valid one-tile PMTiles v3 (raster/PNG, z0/0/0).

    The only way to test the raster branch offline — every published PMTiles in
    the repo is vector.
    """
    import gzip
    import struct

    # One directory entry: tile_id 0 (z0/0/0), run_length 1, at offset 0.
    d = bytearray()
    d += _varint(1)          # entry count
    d += _varint(0)          # tile_id delta (absolute 0)
    d += _varint(1)          # run_length
    d += _varint(len(tile_png))  # length
    d += _varint(1)          # offset encoded as value+1 (offset 0)
    root = gzip.compress(bytes(d))
    meta = gzip.compress(b"{}")

    header = bytearray(127)
    header[0:7] = b"PMTiles"
    header[7] = 3
    root_off = 127
    meta_off = root_off + len(root)
    tile_off = meta_off + len(meta)
    struct.pack_into(
        "<11Q", header, 8,
        root_off, len(root), meta_off, len(meta), 0, 0, tile_off, len(tile_png),
        1, 1, 1,
    )
    header[96] = 1  # clustered
    header[97] = 2  # internal compression = gzip
    header[98] = 1  # tile compression = none (PNG is already compressed)
    header[99] = 2  # tile type = PNG
    header[100] = 0  # min zoom
    header[101] = 0  # max zoom
    with open(path, "wb") as fh:
        fh.write(bytes(header) + root + meta + tile_png)
    return path


def _make_png(path: str, w: int, h: int, rgba=(0.2, 0.5, 0.9, 1.0)) -> str:
    """A solid-colour PNG written by Blender itself — no external deps."""
    img = bpy.data.images.new("rete-test-png", width=w, height=h, alpha=True)
    img.pixels = list(rgba) * (w * h)
    img.filepath_raw = path
    img.file_format = "PNG"
    img.save()
    return path


def _make_mp4(directory: str, frames: int = 5) -> str:
    """A tiny real .mp4 rendered by Blender (bundled FFmpeg)."""
    scene = bpy.context.scene
    scene.render.resolution_x, scene.render.resolution_y = 32, 16
    scene.frame_start, scene.frame_end = 1, frames
    scene.render.image_settings.file_format = "FFMPEG"
    scene.render.ffmpeg.format = "MPEG4"
    scene.render.ffmpeg.codec = "H264"
    scene.render.filepath = os.path.join(directory, "clip")
    bpy.ops.render.render(animation=True)
    for f in os.listdir(directory):
        if f.endswith(".mp4"):
            return os.path.join(directory, f)
    raise AssertionError("Blender did not produce an .mp4")


@test("media: an image URL becomes an upright plane sized to its aspect")
def t_image_plane():
    directory = STATE.get("dir") or tempfile.mkdtemp(prefix="rete-media-")
    STATE["dir"] = directory
    png = _make_png(os.path.join(directory, "wide.png"), 40, 20)  # 2:1
    url = "file://" + png.replace("\\", "/")

    name = fresh_local("img-plane")
    coll = builder.get_collection(name, bpy.context.scene)
    obj = media.image_plane(url, "photo", coll, height=1.0)
    truthy(obj is not None, "image plane created")
    truthy(obj.type == "MESH" and len(obj.data.polygons) == 1, "a single quad")
    bpy.context.view_layer.update()
    truthy(abs(obj.dimensions[0] - 2.0) < 0.05, f"width follows the 2:1 aspect ({obj.dimensions[0]:.2f})")
    truthy(abs(obj.dimensions[2] - 1.0) < 0.05, "height is the requested 1 m")
    truthy(obj.material_slots and obj.material_slots[0].material, "textured")


@test("media: a 2:1 image becomes the world's 360° environment")
def t_world_panorama():
    directory = STATE["dir"]
    png = _make_png(os.path.join(directory, "pano.png"), 64, 32)
    url = "file://" + png.replace("\\", "/")
    ok = media.set_world_panorama(url)
    truthy(ok, "panorama set")
    world = bpy.context.scene.world
    truthy(world and world.use_nodes, "world uses nodes")
    envs = [n for n in world.node_tree.nodes if n.type == "TEX_ENVIRONMENT"]
    truthy(envs and envs[0].image is not None, "environment texture wired")


@test("media: a video URL becomes a movie-textured plane synced to the timeline")
def t_video():
    directory = STATE.get("dir") or tempfile.mkdtemp(prefix="rete-media-")
    STATE["dir"] = directory

    # Detection is FFmpeg-independent and always checked.
    eq(detect.classify_column("v", [engine.Cell("iri", "file:///x/clip.mp4")]), detect.VIDEO)
    eq(detect.classify_column("v", [engine.Cell("iri", "file:///x/clip.webm")]), detect.VIDEO)

    # Some Blender builds ship without FFmpeg and can neither render nor decode a
    # movie; the render attempt is the reliable probe. The add-on must then
    # degrade cleanly rather than crash.
    try:
        mp4 = _make_mp4(directory, frames=5)
    except (TypeError, RuntimeError) as exc:
        result = media.video_plane("file:///does/not/exist.mp4", "clip",
                                   builder.get_collection(fresh_local("video"), bpy.context.scene))
        truthy(result is None, "video degrades to None without FFmpeg")
        print(f"       Blender build lacks FFmpeg ({exc}) — degradation verified")
        return
    url = "file://" + mp4.replace("\\", "/")
    name = fresh_local("video")
    coll = builder.get_collection(name, bpy.context.scene)
    result = media.video_plane(url, "clip", coll, height=1.0, frame_start=1)
    truthy(result is not None, "video plane created")
    obj, frames = result
    eq(frames, 5, "clip length read")
    mat = obj.material_slots[0].material if obj.material_slots else None
    truthy(mat is not None, "video material assigned")
    tex = next((n for n in mat.node_tree.nodes if n.type == "TEX_IMAGE"), None)
    truthy(tex is not None and tex.image.source == "MOVIE", "movie texture")
    truthy(tex.image_user.use_auto_refresh, "plays on the timeline")

    # And through the build path: a graph row with a geo point + a video column.
    nt = (
        '<https://x.org/c> <http://www.w3.org/2000/01/rdf-schema#label> "Clip" .\n'
        '<https://x.org/c> <https://w3id.org/rete/media#video> <%s> .\n'
        '<https://x.org/c> <https://w3id.org/rete/geo3#asWKT3D> "POINT Z(0 0 0)" .\n' % url
    )
    src = os.path.join(directory, "video.rete")
    engine.engine().Builder().add(nt, "nt").export(src)
    r = engine.select(
        src,
        "PREFIX m: <https://w3id.org/rete/media#>\n"
        "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n"
        "PREFIX geo3: <https://w3id.org/rete/geo3#>\n"
        "SELECT ?s ?label ?video ?wkt WHERE { ?s rdfs:label ?label ; m:video ?video ; geo3:asWKT3D ?wkt }",
    )
    eq(detect.classify_result(r)["video"], detect.VIDEO, "m:video detected as video")
    bname = fresh_local("video-build")
    report = builder.build(
        r,
        builder.Settings(source=src, collection_name=bname, scale_mode="M",
                         deep_properties=False, material_mode="NONE", frame_start=1, frame_end=2),
    )
    truthy(report.media >= 1, "video built into the scene")
    # The timeline was extended to fit the 5-frame clip.
    truthy(bpy.context.scene.frame_end >= 5, f"timeline covers the clip ({bpy.context.scene.frame_end})")


@test("PMTiles: a vector .pmtiles builds one mesh per map layer")
def t_pmtiles_vector():
    pmt = os.path.join(REPO, "experiments", "graph-map", "out", "graphmap.pmtiles")
    if not os.path.exists(pmt):
        print("       (graphmap.pmtiles absent — skipping)")
        return

    archive = tiles.PMTiles(pmt)
    eq(archive.type_name(), "mvt", "recognised as vector")
    data = archive.tile(archive.min_zoom, 0, 0)
    truthy(data, "tile 0/0/0 present")
    feats = tiles.decode_tile(data, 0, 0, archive.min_zoom)
    truthy(len(feats) > 50, f"features decoded ({len(feats)})")
    # Coordinates are real lon/lat.
    lon, lat = feats[0].rings[0][0]
    truthy(-180 <= lon <= 180 and -85 <= lat <= 85, f"geographic coords ({lon:.1f},{lat:.1f})")

    url = "file://" + pmt.replace("\\", "/")
    src = os.path.join(STATE["dir"], "map.rete")
    engine.engine().Builder().add(
        f'<https://x.org/d> <https://w3id.org/rete/map#basemap> <{url}> .\n', "nt"
    ).export(src)
    r = engine.select(src, "SELECT ?s ?map WHERE { ?s <https://w3id.org/rete/map#basemap> ?map }")
    eq(detect.classify_result(r)["map"], detect.MAP, "the .pmtiles column is a map")

    name = fresh_local("pmtiles")
    report = builder.build(
        r,
        builder.Settings(source=src, collection_name=name, scale_mode="FIT", fit_size=10.0,
                         map_zoom=-1, map_tiles=40, deep_properties=False, material_mode="NONE"),
    )
    truthy(report.map_layers >= 1, f"map layers built ({report.map_layers})")
    layer_objs = [o for o in bpy.data.collections[name].all_objects if o.get("rete:mapLayer")]
    truthy(layer_objs, "layer objects present")
    verts = sum(len(o.data.vertices) for o in layer_objs)
    truthy(verts > 500, f"real geometry ({verts} verts)")
    truthy(all(o.material_slots and o.material_slots[0].material for o in layer_objs), "layers coloured")
    # Fit-scaled into a ~10 m box.
    bpy.context.view_layer.update()
    biggest = max(max(o.dimensions) for o in layer_objs)
    truthy(2.0 < biggest < 30.0, f"map fit to scene size ({biggest:.1f} m)")
    print(f"       {report.summary()}")


@test("PMTiles: a raster .pmtiles becomes textured tile planes")
def t_pmtiles_raster():
    directory = STATE["dir"]
    png = open(_make_png(os.path.join(directory, "tile.png"), 8, 8, (0.9, 0.3, 0.1, 1.0)), "rb").read()
    pmt = _make_raster_pmtiles(png, os.path.join(directory, "raster.pmtiles"))

    archive = tiles.PMTiles(pmt)
    eq(archive.type_name(), "png", "recognised as raster")
    truthy(archive.tile(0, 0, 0) == png, "the single tile round-trips")

    placement = geometry.Placement(geographic=True, ref_lon=0.0, ref_lat=0.0, scale=1e-5)
    name = fresh_local("raster")
    coll = builder.get_collection(name, bpy.context.scene)
    objs, note = tiles.build_map(
        "file://" + pmt.replace("\\", "/"),
        placement=placement, bbox=(-180, -85, 180, 85), zoom=0, max_tiles=4,
        collection=coll, name="ras",
    )
    truthy(objs, f"raster tile planes built ({note})")
    plane = objs[0]
    truthy(plane.type == "MESH" and len(plane.data.polygons) == 1, "a quad per tile")
    truthy(plane.material_slots and plane.material_slots[0].material, "tile textured")


@test("PMTiles: a remote .pmtiles is read over HTTP range, not downloaded whole")
def t_pmtiles_range():
    import http.server
    import threading

    directory = STATE["dir"]
    png = open(_make_png(os.path.join(directory, "r2.png"), 8, 8), "rb").read()
    pmt_path = _make_raster_pmtiles(png, os.path.join(directory, "range.pmtiles"))
    blob = open(pmt_path, "rb").read()
    served = {"bytes": 0}

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            rng = self.headers.get("Range", "")
            if rng.startswith("bytes="):
                lo, hi = rng[6:].split("-")
                lo, hi = int(lo), int(hi)
                chunk = blob[lo:hi + 1]
                served["bytes"] += len(chunk)
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {lo}-{hi}/{len(blob)}")
                self.send_header("Content-Length", str(len(chunk)))
                self.end_headers()
                self.wfile.write(chunk)
            else:
                served["bytes"] += len(blob)
                self.send_response(200)
                self.send_header("Content-Length", str(len(blob)))
                self.end_headers()
                self.wfile.write(blob)

    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        url = f"http://127.0.0.1:{server.server_address[1]}/range.pmtiles"
        archive = tiles.PMTiles(url)          # header + root dir only
        eq(archive.type_name(), "png", "remote header parsed over range")
        tile = archive.tile(0, 0, 0)
        eq(tile, png, "remote tile fetched over range")
        truthy(served["bytes"] < len(blob), f"read {served['bytes']} of {len(blob)} bytes, not the whole file")
    finally:
        server.shutdown()


def _make_dot_splat(path: str, n: int = 40) -> str:
    """A minimal antimatter15 .splat: n × 32-byte records."""
    import struct

    with open(path, "wb") as fh:
        for i in range(n):
            fh.write(struct.pack("<fff", i * 0.1, i * 0.2, i * 0.05))  # position
            fh.write(struct.pack("<fff", 0.01, 0.01, 0.01))            # scale
            fh.write(bytes((i % 256, (2 * i) % 256, (3 * i) % 256, 255)))  # rgba
            fh.write(bytes((128, 128, 128, 255)))                     # rotation
    return path


def _make_splat_ply(path: str, n: int = 30) -> str:
    """A minimal binary 3DGS .ply with the tell-tale f_dc/scale/rot properties."""
    import struct

    props = ["x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity",
             "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3"]
    header = "ply\nformat binary_little_endian 1.0\n"
    header += f"element vertex {n}\n"
    header += "".join(f"property float {p}\n" for p in props)
    header += "end_header\n"
    with open(path, "wb") as fh:
        fh.write(header.encode("ascii"))
        for i in range(n):
            row = [i * 0.1, i * 0.2, i * 0.05,   # xyz
                   0.5, -0.2, 0.1,               # f_dc (colour)
                   3.0,                          # opacity (sigmoid → ~0.95)
                   0.01, 0.01, 0.01,             # scale
                   1.0, 0.0, 0.0, 0.0]           # rot quaternion
            fh.write(struct.pack("<14f", *row))
    return path


def _make_mesh_ply(path: str) -> str:
    """A plain (non-splat) PLY — no f_dc/scale/rot properties."""
    header = ("ply\nformat binary_little_endian 1.0\nelement vertex 1\n"
              "property float x\nproperty float y\nproperty float z\n"
              "property uchar red\nproperty uchar green\nproperty uchar blue\n"
              "element face 0\nproperty list uchar int vertex_indices\nend_header\n")
    import struct

    with open(path, "wb") as fh:
        fh.write(header.encode("ascii"))
        fh.write(struct.pack("<fff", 0.0, 0.0, 0.0) + bytes((255, 0, 0)))
    return path


@test("splats: .ply headers are told apart; .splat/.ply parse into preview points")
def t_splat_parse():
    directory = STATE.get("dir") or tempfile.mkdtemp(prefix="rete-splat-")
    STATE["dir"] = directory

    splat_ply = _make_splat_ply(os.path.join(directory, "s.ply"), 30)
    mesh_ply = _make_mesh_ply(os.path.join(directory, "m.ply"))
    dot = _make_dot_splat(os.path.join(directory, "s.splat"), 40)

    truthy(splats.is_splat_ply(splat_ply), "3DGS .ply recognised as a splat")
    truthy(not splats.is_splat_ply(mesh_ply), "plain mesh .ply is not a splat")
    truthy(assets.is_splat_asset("file://" + dot.replace("\\", "/")), ".splat is a splat asset")
    truthy(
        assets.is_splat_asset("file://" + splat_ply.replace("\\", "/")),
        "splat .ply is a splat asset (sniffed)",
    )
    truthy(
        not assets.is_splat_asset("file://" + mesh_ply.replace("\\", "/")),
        "mesh .ply is not routed to splats",
    )

    pos, col = splats.parse_dot_splat(dot)
    eq(len(pos), 40, ".splat points parsed")
    eq(len(col), 40, ".splat colours parsed")
    pos2, col2 = splats.parse_ply_splat(splat_ply)
    eq(len(pos2), 30, ".ply splat points parsed")
    # f_dc 0.5 → colour 0.5 + C0*0.5 ≈ 0.64; opacity sigmoid(3) ≈ 0.95.
    truthy(0.5 < col2[0][0] < 0.8, f"SH colour decoded ({col2[0][0]:.2f})")
    truthy(col2[0][3] > 0.9, "opacity decoded")


@test("splats: no add-on → an honest centred point-cloud preview, with a note")
def t_splat_preview():
    directory = STATE["dir"]
    dot = _make_dot_splat(os.path.join(directory, "prev.splat"), 50)
    url = "file://" + dot.replace("\\", "/")

    truthy(splats.find_splat_importer() is None, "no 3DGS add-on registered in a clean Blender")
    objects, note, via_addon = assets.import_splat_asset(url, refresh=True)
    truthy(not via_addon, "fell back to preview")
    eq(len(objects), 1, "one preview object")
    obj = objects[0]
    eq(len(obj.data.vertices), 50, "a vertex per Gaussian")
    truthy("splat_color" in obj.data.attributes, "colour attribute present")
    truthy(obj.get("rete:splatPreview"), "flagged as a preview")
    truthy("3DGS add-on" in note or "add-on" in note, "note points at the add-on")
    # Centred on its centroid, so it lands cleanly when placed.
    xs = [v.co.x for v in obj.data.vertices]
    truthy(abs(sum(xs) / len(xs)) < 1e-4, "preview centred on its centroid")


@test("splats: a .ksplat with no add-on degrades with a convert message")
def t_splat_ksplat():
    directory = STATE["dir"]
    path = os.path.join(directory, "web.ksplat")
    with open(path, "wb") as fh:
        fh.write(b"KSPL" + bytes(64))  # not parseable, and no add-on
    try:
        assets.import_splat_asset("file://" + path.replace("\\", "/"), refresh=True)
        raise AssertionError("expected an IOError for .ksplat without an add-on")
    except IOError as exc:
        truthy(".ply" in str(exc) or "convert" in str(exc), "message suggests converting to .ply")


@test("splats: a registered 3DGS operator is discovered and used")
def t_splat_addon_handoff():
    calls = {"n": 0}

    class RETE_OT_fake_3dgs_import(bpy.types.Operator):
        bl_idname = "object.rete_fake_3dgs_import"
        bl_label = "Fake 3DGS import"

        def execute(self, context):
            calls["n"] += 1
            obj = bpy.data.objects.new("fake-splat", None)
            context.scene.collection.objects.link(obj)
            return {"FINISHED"}

    # This module uses `from __future__ import annotations`, so a class-body
    # `filepath: StringProperty(...)` would stringize and never register. Set the
    # real property object explicitly.
    RETE_OT_fake_3dgs_import.__annotations__ = {
        "filepath": bpy.props.StringProperty(subtype="FILE_PATH")
    }
    bpy.utils.register_class(RETE_OT_fake_3dgs_import)
    try:
        op = splats.find_splat_importer()
        truthy(op is not None, "discovery found the 3DGS importer")
        directory = STATE["dir"]
        dot = _make_dot_splat(os.path.join(directory, "handoff.splat"), 10)
        objects, note, via_addon = assets.import_splat_asset(
            "file://" + dot.replace("\\", "/"), refresh=True
        )
        truthy(via_addon, "handoff reported")
        truthy(calls["n"] >= 1, "the add-on operator was actually called")
        truthy(any(o.name.startswith("fake-splat") for o in objects), "add-on objects returned")
        truthy(all(o.get("rete:splat") for o in objects), "tagged as splats")
    finally:
        bpy.utils.unregister_class(RETE_OT_fake_3dgs_import)
        # Clear the cached import so later tests re-resolve without the add-on.
        assets._splat_sniff.clear()


@test("splats: build path wraps the splat in an empty, never mutating its matrix")
def t_splat_build():
    directory = STATE["dir"]
    dot = _make_dot_splat(os.path.join(directory, "build.splat"), 60)
    url = "file://" + dot.replace("\\", "/")

    nt = (
        '<https://x.org/o> <http://www.w3.org/2000/01/rdf-schema#label> "Object" .\n'
        '<https://x.org/o> <https://w3id.org/rete/media#splat> <%s> .\n'
        '<https://x.org/o> <https://w3id.org/rete/geo3#asWKT3D> "POINT Z(5000 0 0)" .\n' % url
    )
    src = os.path.join(directory, "splat.rete")
    engine.engine().Builder().add(nt, "nt").export(src)
    r = engine.select(
        src,
        "PREFIX m: <https://w3id.org/rete/media#>\n"
        "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n"
        "PREFIX geo3: <https://w3id.org/rete/geo3#>\n"
        "SELECT ?s ?label ?splat ?wkt WHERE { ?s rdfs:label ?label ; m:splat ?splat ; geo3:asWKT3D ?wkt }",
    )
    eq(detect.classify_result(r)["splat"], detect.SPLAT, "m:splat detected as a splat")

    name = fresh_local("splat-build")
    report = builder.build(
        r,
        builder.Settings(source=src, collection_name=name, scale_mode="MM", recentre=False,
                         deep_properties=False, material_mode="NONE", layout="GEOMETRY"),
    )
    truthy(report.assets >= 1, "splat counted as an imported asset")
    coll = bpy.data.collections[name]
    empties = [o for o in coll.all_objects if o.type == "EMPTY" and o.get("rete:splatGroup")]
    truthy(empties, "splat wrapped in an empty")
    empty = empties[0]
    bpy.context.view_layer.update()
    # POINT Z(5000 0 0) in mm → 5 m along X: the EMPTY carries the placement.
    truthy(abs(empty.location[0] - 5.0) < 1e-3, f"empty placed at the row position ({empty.location[0]:.2f})")
    splat = next((c for c in empty.children if c.get("rete:splat")), None)
    truthy(splat is not None, "splat parented to the empty")
    # The splat's OWN matrix must be untouched (identity basis) — the whole point.
    truthy(splat.matrix_basis == _identity_matrix(), "splat's own transform never mutated")
    truthy("splat_color" in splat.data.attributes, "preview colour survived")


def _identity_matrix():
    import mathutils

    return mathutils.Matrix.Identity(4)


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
        t_image_plane, t_world_panorama, t_video,
        t_pmtiles_vector, t_pmtiles_raster, t_pmtiles_range,
        t_splat_parse, t_splat_preview, t_splat_ksplat, t_splat_addon_handoff, t_splat_build,
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
