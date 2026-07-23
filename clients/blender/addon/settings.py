"""Scene-level settings, and the collections the UI browses.

Kept separate from the operators so that :mod:`.builder` can be driven from a
plain script with a :class:`builder.Settings` object, while the UI edits a
Blender ``PropertyGroup`` that knows how to convert itself into one.
"""

from __future__ import annotations

from typing import Dict

import bpy
from bpy.props import (
    BoolProperty,
    CollectionProperty,
    EnumProperty,
    FloatProperty,
    IntProperty,
    PointerProperty,
    StringProperty,
)
from bpy.types import PropertyGroup

from . import builder, detect, physics, relations

MATERIAL_MODES = (
    ("AUTO", "Auto", "Colour by whatever the result offers, best first"),
    ("CLASS", "By class", "One colour per rdf:type"),
    ("NUMBER", "By number", "A perceptual ramp across a numeric column"),
    ("COLOR", "From colour column", "Use a colour literal in the result"),
    ("TEXTURE", "From image", "Texture each object with its image URL"),
    ("NONE", "None", "Leave materials alone"),
)

PHYSICS_MODES = (
    ("NONE", "None", "No physics"),
    ("ACTIVE", "Active bodies", "Objects fall and collide"),
    ("PASSIVE", "Passive bodies", "Objects collide but do not move"),
)

IMAGE_MODES = (
    ("MATERIAL", "Material", "Texture the row's object with the image"),
    ("PLANE", "Image plane", "Stand the image upright in 3D at the row's position"),
    ("WORLD", "360° world", "Use the first image as the scene's world environment"),
)

MAP_MODES = (
    ("AUTO", "Auto", "Vector or raster, from the file"),
    ("VECTOR", "Vector", "Decode MVT features into meshes"),
    ("RASTER", "Raster", "Lay tile images onto planes"),
)

AXIS_UP = (
    ("Z", "Z up", "The data is already Z-up, like Blender"),
    ("Y", "Y up", "The data is Y-up (glTF convention) and needs converting"),
)

EXPORT_SCOPES = (
    ("SCENE", "Whole scene", "Every object in the scene"),
    ("GRAPH", "Graph objects", "Only objects that carry a graph IRI"),
    ("COLLECTION", "Collection", "One collection"),
    ("SELECTED", "Selected", "The current selection"),
)


class ReteColumn(PropertyGroup):
    """One variable in the current result, and the role it plays."""

    name: StringProperty(name="Variable")
    detected: StringProperty(name="Detected")
    sample: StringProperty(name="Sample")
    role: EnumProperty(
        name="Role",
        description="What this column means when the scene is built",
        items=detect.ROLE_ITEMS,
        default="AUTO",
    )


class ReteExample(PropertyGroup):
    """A ready-made query the dataset ships with."""

    title: StringProperty()
    question: StringProperty()
    sparql: StringProperty()


class RetePredicate(PropertyGroup):
    """A predicate used by the imported entities, for the relation pickers."""

    name: StringProperty()
    count: IntProperty()


class ReteSettings(PropertyGroup):
    """Everything the add-on remembers, saved with the .blend."""

    # -- the graph ---------------------------------------------------------
    source: StringProperty(
        name="Graph",
        description="A .rete file: a local path, or an https URL read lazily over HTTP range requests",
        default="https://data.graphplaza.com/z-anatomy/z-anatomy.rete",
    )
    query: PointerProperty(
        name="Query",
        description="The text block holding the SPARQL query (edit it in the Text Editor)",
        type=bpy.types.Text,
    )
    reason: BoolProperty(
        name="OWL reasoning",
        description="Answer with OWL 2 QL entailment, using the ontology inside the file",
        default=False,
    )

    # -- state -------------------------------------------------------------
    status: StringProperty(name="Status", default="")
    card_title: StringProperty(default="")
    card_license: StringProperty(default="")
    card_counts: StringProperty(default="")
    fetched: StringProperty(default="")
    row_count: IntProperty(default=0)
    columns: CollectionProperty(type=ReteColumn)
    column_index: IntProperty(default=0)
    examples: CollectionProperty(type=ReteExample)
    example_index: IntProperty(default=0)
    predicates: CollectionProperty(type=RetePredicate)

    # -- what to build -----------------------------------------------------
    collection_name: StringProperty(name="Collection", default="rete")
    limit: IntProperty(
        name="Max rows",
        description="How many result rows become objects",
        default=2000, min=1, soft_max=50000,
    )
    point_style: EnumProperty(
        name="Marker", items=builder.POINT_STYLES, default="SPHERE",
        description="What to create for a row that has no 3D asset",
    )
    point_size: FloatProperty(name="Marker size", default=0.05, min=0.0001, soft_max=2.0, unit="LENGTH")
    layout: EnumProperty(name="Layout", items=builder.LAYOUTS, default="AUTO")
    layout_spacing: FloatProperty(name="Spacing", default=1.0, min=0.0, soft_max=20.0)
    scatter_x: StringProperty(name="X")
    scatter_y: StringProperty(name="Y")
    scatter_z: StringProperty(name="Z")

    # -- placement ---------------------------------------------------------
    scale_mode: EnumProperty(name="Scale", items=builder.SCALE_MODES, default="FIT")
    fit_size: FloatProperty(name="Fit to", default=10.0, min=0.001, soft_max=1000.0, unit="LENGTH")
    custom_scale: FloatProperty(name="Factor", default=1.0, soft_min=0.000001, soft_max=1000.0)
    axis_up: EnumProperty(name="Up axis", items=AXIS_UP, default="Z")
    flip_x: BoolProperty(
        name="Flip X",
        description="Mirror on X — anatomical data uses +X = the subject's left",
        default=False,
    )
    recentre: BoolProperty(name="Recentre", default=True, description="Move the result to the world origin")
    extrude: FloatProperty(
        name="Extrude",
        description="Give flat polygons height, in metres — footprints become massing",
        default=0.0, min=0.0, soft_max=100.0, unit="LENGTH",
    )

    # -- assets ------------------------------------------------------------
    import_assets: BoolProperty(name="Import 3D assets", default=True)
    max_assets: IntProperty(
        name="Asset limit",
        description="Stop importing assets after this many rows; the rest become markers",
        default=400, min=0, soft_max=5000,
    )
    texture_size: IntProperty(name="Texture size", default=2048, min=64, max=8192)

    # -- properties --------------------------------------------------------
    deep_properties: BoolProperty(
        name="Inherit all properties",
        description=(
            "Fetch every statement about each imported entity and write them as "
            "custom properties — drivable, and readable by Geometry Nodes"
        ),
        default=True,
    )

    # -- materials ---------------------------------------------------------
    material_mode: EnumProperty(name="Colour by", items=MATERIAL_MODES, default="AUTO")
    material_var: StringProperty(
        name="Column",
        description=(
            "Which column drives the colour. A numeric column becomes a "
            "perceptual ramp; anything else becomes one colour per distinct value"
        ),
    )

    # -- media (images, video) --------------------------------------------
    image_mode: EnumProperty(
        name="Images", items=IMAGE_MODES, default="MATERIAL",
        description="How an image column is used",
    )
    media_height: FloatProperty(
        name="Screen height", default=1.0, min=0.01, soft_max=50.0, unit="LENGTH",
        description="Height of image and video planes, in metres",
    )

    # -- maps (PMTiles) ----------------------------------------------------
    map_mode: EnumProperty(name="Map", items=MAP_MODES, default="AUTO")
    map_zoom: IntProperty(
        name="Zoom", default=-1, min=-1, max=22,
        description="Tile zoom level; -1 picks the highest that fits the tile budget",
    )
    map_tiles: IntProperty(
        name="Tile budget", default=40, min=1, soft_max=400,
        description="Maximum tiles to read for the chosen extent",
    )
    map_extrude: FloatProperty(
        name="Extrude", default=0.0, min=0.0, soft_max=1000.0, unit="LENGTH",
        description="Give map polygons height, in metres — footprints become massing",
    )

    # -- time --------------------------------------------------------------
    time_mode: EnumProperty(name="Time", items=builder.TIME_MODES, default="NONE")
    frame_start: IntProperty(name="Start frame", default=1, min=0)
    frame_end: IntProperty(name="End frame", default=250, min=1)
    time_span: StringProperty(default="")

    # -- relations ---------------------------------------------------------
    relation_mode: EnumProperty(name="Relations", items=relations.MODES, default="NONE")
    relation_predicate: StringProperty(name="Predicate")
    relation_inverse: BoolProperty(name="Invert", default=False)

    # -- physics -----------------------------------------------------------
    physics_mode: EnumProperty(name="Bodies", items=PHYSICS_MODES, default="NONE")
    physics_shape: EnumProperty(name="Shape", items=physics.SHAPES, default="CONVEX_HULL")
    physics_mass_var: StringProperty(name="Mass from")
    constraint_predicate: StringProperty(name="Link predicate")
    constraint_type: EnumProperty(name="Link type", items=physics.CONSTRAINT_TYPES, default="FIXED")

    # -- scale path --------------------------------------------------------
    point_cloud: BoolProperty(
        name="As point cloud",
        description=(
            "Write the whole result into one attributed mesh for Geometry Nodes "
            "instead of one object per row — the way to handle very large results"
        ),
        default=False,
    )

    # -- export ------------------------------------------------------------
    export_scope: EnumProperty(name="Export", items=EXPORT_SCOPES, default="GRAPH")
    export_collection: StringProperty(name="From collection")
    export_base: StringProperty(name="Base IRI", default="https://example.org/scene/")
    export_title: StringProperty(name="Title", default="Blender scene")
    export_path: StringProperty(name="File", subtype="FILE_PATH", default="//scene.rete")
    export_keep_iris: BoolProperty(
        name="Keep source IRIs", default=True,
        description="Objects imported from a graph keep their original identity",
    )

    # ----------------------------------------------------------------- glue

    def query_text(self) -> str:
        return self.query.as_string() if self.query else ""

    def overrides(self) -> Dict[str, str]:
        return {c.name: c.role for c in self.columns if c.role != "AUTO"}

    def to_build_settings(self) -> "builder.Settings":
        return builder.Settings(
            source=self.source,
            query=self.query_text(),
            collection_name=self.collection_name or "rete",
            limit=self.limit,
            point_style=self.point_style,
            point_size=self.point_size,
            layout=self.layout,
            layout_spacing=self.layout_spacing,
            scatter_vars=(self.scatter_x, self.scatter_y, self.scatter_z),
            scale_mode=self.scale_mode,
            fit_size=self.fit_size,
            custom_scale=self.custom_scale,
            axis_up=self.axis_up,
            flip_x=self.flip_x,
            recentre=self.recentre,
            extrude=self.extrude,
            import_assets=self.import_assets,
            max_assets=self.max_assets,
            texture_size=self.texture_size,
            deep_properties=self.deep_properties,
            reason=self.reason,
            material_mode=self.material_mode,
            material_var=self.material_var,
            image_mode=self.image_mode,
            media_height=self.media_height,
            map_mode=self.map_mode,
            map_zoom=self.map_zoom,
            map_tiles=self.map_tiles,
            map_extrude=self.map_extrude,
            time_mode=self.time_mode,
            frame_start=self.frame_start,
            frame_end=self.frame_end,
            relation_mode=self.relation_mode,
            relation_predicate=self.relation_predicate,
            relation_inverse=self.relation_inverse,
            physics_mode=self.physics_mode,
            physics_shape=self.physics_shape,
            physics_mass_var=self.physics_mass_var,
            constraint_predicate=self.constraint_predicate,
            constraint_type=self.constraint_type,
            point_cloud=self.point_cloud,
            overrides=self.overrides(),
        )


CLASSES = (ReteColumn, ReteExample, RetePredicate, ReteSettings)


def register() -> None:
    for cls in CLASSES:
        bpy.utils.register_class(cls)
    bpy.types.Scene.rete = PointerProperty(type=ReteSettings)


def unregister() -> None:
    del bpy.types.Scene.rete
    for cls in reversed(CLASSES):
        bpy.utils.unregister_class(cls)
