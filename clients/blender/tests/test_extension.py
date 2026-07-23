"""Verify the *packaged* extension installs and works.

The other suites import the add-on from source, with the engine already present
in Blender's Python. This one installs the built ``.zip`` into a throwaway
Blender profile and checks that the bundled wheel is what makes it work — the
thing a user actually experiences.

    blender -b --command extension install-file -r user_default --enable dist/rete-*.zip
    blender -b --python clients/blender/tests/test_extension.py
"""

from __future__ import annotations

import os
import sys
import tempfile

import bpy

MODULE = "bl_ext.user_default.rete"
FAILURES = []


def check(condition, message):
    if condition:
        print(f"  ok   {message}")
    else:
        print(f"  FAIL {message}")
        FAILURES.append(message)


def main() -> None:
    print("\nrete Blender add-on — packaged extension")
    print(f"Blender {bpy.app.version_string}\n")

    import addon_utils

    enabled = {m.__name__ for m in addon_utils.modules() if addon_utils.check(m.__name__)[1]}
    check(MODULE in enabled or MODULE in sys.modules, f"{MODULE} is enabled")

    # The engine must come from the bundled wheel, not from the environment.
    import rete_graph

    location = os.path.dirname(rete_graph.__file__)
    print(f"       engine {rete_graph.__version__} from {location}")
    check("__init__" not in location, "engine importable")

    settings = getattr(bpy.context.scene, "rete", None)
    check(settings is not None, "scene settings registered by the installed extension")
    check(hasattr(bpy.types, "RETE_PT_graph"), "sidebar panel registered")
    check(hasattr(bpy.types, "RETE_OT_build_scene"), "build operator registered")
    check("rete" in bpy.app.driver_namespace, "driver function registered")

    # A full round trip through the installed code path.
    module = sys.modules.get(MODULE)
    check(module is not None, "the extension module is importable")
    if module is None:
        sys.exit(1)

    directory = tempfile.mkdtemp(prefix="rete-ext-")
    path = os.path.join(directory, "fixture.rete")
    nt = (
        '<https://x.org/a> <http://www.w3.org/2000/01/rdf-schema#label> "Alpha" .\n'
        '<https://x.org/a> <https://w3id.org/rete/geo3#asWKT3D> "POINT Z(0 0 0)" .\n'
        '<https://x.org/b> <http://www.w3.org/2000/01/rdf-schema#label> "Beta" .\n'
        '<https://x.org/b> <https://w3id.org/rete/geo3#asWKT3D> "POINT Z(1000 0 0)" .\n'
    )
    rete_graph.Builder().add(nt, "nt").export(path)

    result = module.engine.select(
        path,
        """PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
           PREFIX geo3: <https://w3id.org/rete/geo3#>
           SELECT ?s ?label ?wkt WHERE { ?s rdfs:label ?label ; geo3:asWKT3D ?wkt }
           ORDER BY ?label""",
    )
    check(len(result) == 2, "the installed extension queries a .rete file")

    report = module.builder.build(
        result,
        module.builder.Settings(
            source=path,
            collection_name="ext-test",
            scale_mode="MM",
            recentre=False,
            deep_properties=True,
            layout="GEOMETRY",
        ),
    )
    check(report.objects == 2, f"scene built through the installed code ({report.objects} objects)")
    beta = next(
        (o for o in bpy.data.collections["ext-test"].objects if o.get("rete:iri", "").endswith("/b")),
        None,
    )
    check(beta is not None, "objects carry their graph identity")
    if beta is not None:
        check(abs(beta.location[0] - 1.0) < 1e-3, "millimetre coordinates placed correctly")

    # And the operators are callable, which is what the buttons do.
    settings.source = path
    status = bpy.ops.rete.open_graph()
    check("FINISHED" in status, "the Open graph operator runs")

    print(f"\n{len(FAILURES)} failed")
    if FAILURES:
        sys.exit(1)
    print("PACKAGED EXTENSION OK")


if __name__ == "__main__":
    main()
