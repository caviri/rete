"""rete for Blender — knowledge graphs as scenes.

Opens a ``.rete`` file (local, or remote and read lazily over HTTP range
requests), runs SPARQL against it, and turns the answer into scene content: 3D
assets imported, geometry placed, every RDF property inherited as a drivable
custom property, relations expressed as hierarchy or physical constraints, and
time mapped onto the timeline. Scenes go back out as new ``.rete`` files.

The add-on registers under the "rete" tab in the 3D viewport's sidebar (press N).
"""

bl_info = {
    "name": "rete — knowledge graphs in 3D",
    "author": "Carlos Vivar Rios",
    "version": (0, 1, 0),
    "blender": (4, 2, 0),
    "location": "3D Viewport ▸ Sidebar ▸ rete",
    "description": (
        "Query .rete knowledge graphs with SPARQL and build scenes from the "
        "results: assets, geometry, inherited properties, time, and physics."
    ),
    "doc_url": "https://caviri.github.io/rete/blender.html",
    "tracker_url": "https://github.com/caviri/rete/issues",
    "support": "COMMUNITY",
    "category": "Import-Export",
}

from . import drivers, ops, settings, ui

#: Registered in dependency order; unregistered in reverse.
_MODULES = (settings, ops, ui, drivers)


def register() -> None:
    for module in _MODULES:
        module.register()


def unregister() -> None:
    for module in reversed(_MODULES):
        module.unregister()


if __name__ == "__main__":  # `blender --python addon/__init__.py` for a quick try
    register()
