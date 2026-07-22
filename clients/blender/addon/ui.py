"""The panels, in the 3D viewport's sidebar under a "rete" tab.

Ordered as the work actually goes: open a graph, ask it something, check what
came back, then build — with the optional passes (time, relations, physics,
export) as their own collapsed sections underneath.
"""

from __future__ import annotations

import bpy
from bpy.types import Panel, UIList

from . import detect, engine, ops
from . import props as rprops

CATEGORY = "rete"


class RETE_UL_columns(UIList):
    """The result's columns, with the role each one plays."""

    def draw_item(self, context, layout, data, item, icon, active_data, active_prop, index):
        row = layout.row(align=True)
        row.label(text=item.name, icon="RNA")
        sub = row.row(align=True)
        sub.scale_x = 0.9
        sub.prop(item, "role", text="")
        detected = row.row()
        detected.enabled = False
        detected.label(text=item.detected.replace("_", " ").lower())


class RETE_UL_examples(UIList):
    """The queries the dataset ships with."""

    def draw_item(self, context, layout, data, item, icon, active_data, active_prop, index):
        row = layout.row(align=True)
        row.label(text=item.title, icon="TEXT")
        op = row.operator(ops.RETE_OT_load_example.bl_idname, text="", icon="IMPORT")
        op.index = index


class RetePanel:
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = CATEGORY


class RETE_PT_graph(RetePanel, Panel):
    bl_idname = "RETE_PT_graph"
    bl_label = "Graph"

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete

        if not engine.available():
            box = layout.box()
            box.alert = True
            box.label(text="The rete engine is not installed", icon="ERROR")
            column = box.column(align=True)
            column.scale_y = 0.8
            for line in _wrap(engine.unavailable_reason(), 44):
                column.label(text=line)
            return

        column = layout.column(align=True)
        column.prop(settings, "source", text="")
        row = column.row(align=True)
        row.operator(ops.RETE_OT_open_graph.bl_idname, icon="FILE_REFRESH")
        row.operator(ops.RETE_OT_pick_file.bl_idname, text="", icon="FILEBROWSER")
        row.menu(ops.RETE_MT_catalog.bl_idname, text="", icon="PRESET")

        if settings.card_title:
            box = layout.box()
            box.label(text=settings.card_title, icon="INFO")
            sub = box.column(align=True)
            sub.scale_y = 0.8
            if settings.card_counts:
                sub.label(text=settings.card_counts)
            if settings.card_license:
                sub.label(text=settings.card_license)
            if settings.fetched:
                sub.label(text=f"fetched {settings.fetched}", icon="URL")


class RETE_PT_query(RetePanel, Panel):
    bl_idname = "RETE_PT_query"
    bl_label = "Query"
    bl_parent_id = "RETE_PT_graph"

    @classmethod
    def poll(cls, context):
        return engine.available()

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete

        row = layout.row(align=True)
        row.prop(settings, "query", text="")
        row.operator(ops.RETE_OT_new_query.bl_idname, text="", icon="ADD")

        row = layout.row(align=True)
        row.scale_y = 1.3
        row.operator(ops.RETE_OT_run_query.bl_idname, icon="PLAY")
        row.prop(settings, "reason", text="", icon="LIGHT", toggle=True)

        if settings.status:
            layout.label(text=settings.status, icon="CHECKMARK")

        if settings.examples:
            layout.label(text="Examples from the file:")
            layout.template_list(
                "RETE_UL_examples", "", settings, "examples", settings, "example_index", rows=3
            )
            if 0 <= settings.example_index < len(settings.examples):
                question = settings.examples[settings.example_index].question
                if question:
                    column = layout.column(align=True)
                    column.scale_y = 0.8
                    for line in _wrap(question, 46):
                        column.label(text=line)


class RETE_PT_columns(RetePanel, Panel):
    bl_idname = "RETE_PT_columns"
    bl_label = "Columns"
    bl_parent_id = "RETE_PT_graph"

    @classmethod
    def poll(cls, context):
        return engine.available() and len(context.scene.rete.columns) > 0

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete
        layout.template_list(
            "RETE_UL_columns", "", settings, "columns", settings, "column_index", rows=4
        )
        if 0 <= settings.column_index < len(settings.columns):
            column = settings.columns[settings.column_index]
            box = layout.box()
            box.scale_y = 0.8
            for line in _wrap(column.sample, 46):
                box.label(text=line)


class RETE_PT_build(RetePanel, Panel):
    bl_idname = "RETE_PT_build"
    bl_label = "Build"

    @classmethod
    def poll(cls, context):
        return engine.available()

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete

        row = layout.row()
        row.scale_y = 1.5
        row.operator(ops.RETE_OT_build_scene.bl_idname, icon="OUTLINER_OB_GROUP_INSTANCE")

        column = layout.column(align=True)
        column.prop(settings, "collection_name")
        column.prop(settings, "limit")

        layout.separator()
        column = layout.column(align=True)
        column.prop(settings, "layout")
        if settings.layout in ("GRID", "CIRCLE", "AUTO"):
            column.prop(settings, "layout_spacing")
        if settings.layout == "SCATTER":
            row = column.row(align=True)
            row.prop_search(settings, "scatter_x", settings, "columns", text="X", icon="EMPTY_ARROWS")
            row = column.row(align=True)
            row.prop_search(settings, "scatter_y", settings, "columns", text="Y", icon="EMPTY_ARROWS")
            row = column.row(align=True)
            row.prop_search(settings, "scatter_z", settings, "columns", text="Z", icon="EMPTY_ARROWS")

        column = layout.column(align=True)
        column.prop(settings, "point_style")
        if settings.point_style != "NONE":
            column.prop(settings, "point_size")


class RETE_PT_placement(RetePanel, Panel):
    bl_idname = "RETE_PT_placement"
    bl_label = "Placement"
    bl_parent_id = "RETE_PT_build"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete
        column = layout.column(align=True)
        column.prop(settings, "scale_mode")
        if settings.scale_mode == "FIT":
            column.prop(settings, "fit_size")
        elif settings.scale_mode == "CUSTOM":
            column.prop(settings, "custom_scale")
        column.prop(settings, "axis_up")
        row = column.row(align=True)
        row.prop(settings, "flip_x", toggle=True)
        row.prop(settings, "recentre", toggle=True)
        column.prop(settings, "extrude")


class RETE_PT_assets(RetePanel, Panel):
    bl_idname = "RETE_PT_assets"
    bl_label = "Assets & properties"
    bl_parent_id = "RETE_PT_build"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete
        column = layout.column(align=True)
        column.prop(settings, "import_assets")
        sub = column.column(align=True)
        sub.enabled = settings.import_assets
        sub.prop(settings, "max_assets")

        layout.separator()
        column = layout.column(align=True)
        column.prop(settings, "deep_properties")
        column.prop(settings, "point_cloud")

        layout.separator()
        column = layout.column(align=True)
        column.prop(settings, "material_mode")
        if settings.material_mode in ("AUTO", "NUMBER", "CLASS"):
            column.prop_search(settings, "material_var", settings, "columns", text="Column")
        if settings.material_mode in ("AUTO", "TEXTURE"):
            column.prop(settings, "texture_size")

        layout.separator()
        layout.operator(ops.RETE_OT_clear_cache.bl_idname, icon="TRASH")


class RETE_PT_time(RetePanel, Panel):
    bl_idname = "RETE_PT_time"
    bl_label = "Time"
    bl_parent_id = "RETE_PT_build"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete
        column = layout.column(align=True)
        column.prop(settings, "time_mode")
        if settings.time_mode != "NONE":
            row = column.row(align=True)
            row.prop(settings, "frame_start")
            row.prop(settings, "frame_end")
            if settings.time_span:
                info = column.column()
                info.enabled = False
                info.label(text=settings.time_span)


class RETE_PT_relations(RetePanel, Panel):
    bl_idname = "RETE_PT_relations"
    bl_label = "Relations"
    bl_parent_id = "RETE_PT_build"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete
        column = layout.column(align=True)
        column.prop(settings, "relation_mode")
        if settings.relation_mode != "NONE":
            row = column.row(align=True)
            row.prop_search(settings, "relation_predicate", settings, "predicates", text="")
            row.operator(ops.RETE_OT_discover_predicates.bl_idname, text="", icon="VIEWZOOM")
            column.prop(settings, "relation_inverse")


class RETE_PT_physics(RetePanel, Panel):
    bl_idname = "RETE_PT_physics"
    bl_label = "Physics"
    bl_parent_id = "RETE_PT_build"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete
        column = layout.column(align=True)
        column.prop(settings, "physics_mode")
        if settings.physics_mode == "NONE":
            return
        column.prop(settings, "physics_shape")
        column.prop_search(settings, "physics_mass_var", settings, "columns", text="Mass from")

        layout.separator()
        box = layout.box()
        box.label(text="Relations as constraints", icon="CONSTRAINT")
        sub = box.column(align=True)
        row = sub.row(align=True)
        row.prop_search(settings, "constraint_predicate", settings, "predicates", text="")
        row.operator(ops.RETE_OT_discover_predicates.bl_idname, text="", icon="VIEWZOOM")
        sub.prop(settings, "constraint_type")


class RETE_PT_selection(RetePanel, Panel):
    bl_idname = "RETE_PT_selection"
    bl_label = "Selected entity"

    @classmethod
    def poll(cls, context):
        return engine.available()

    def draw(self, context):
        layout = self.layout
        obj = context.active_object
        iri = rprops.iri_of(obj) if obj else ""

        if not iri:
            column = layout.column()
            column.enabled = False
            column.label(text="No graph entity selected")
            layout.operator(ops.RETE_OT_select_by_query.bl_idname, icon="RESTRICT_SELECT_OFF")
            return

        box = layout.box()
        box.label(text=obj.name, icon="OBJECT_DATA")
        column = box.column(align=True)
        column.scale_y = 0.8
        for line in _wrap(iri, 46):
            column.label(text=line)
        for klass in rprops.classes_of(obj)[:4]:
            column.label(text=rprops.local_name(klass), icon="DOT")

        row = layout.row(align=True)
        row.operator(ops.RETE_OT_describe_active.bl_idname, icon="FILE_REFRESH")
        row.operator(ops.RETE_OT_expand_selection.bl_idname, icon="OUTLINER_OB_LIGHTPROBE")
        layout.operator(ops.RETE_OT_select_by_query.bl_idname, icon="RESTRICT_SELECT_OFF")

        keys = rprops.user_keys(obj)
        if keys:
            box = layout.box()
            box.label(text=f"{len(keys)} inherited properties", icon="PROPERTIES")
            column = box.column(align=True)
            column.scale_y = 0.8
            for key in keys[:12]:
                values = rprops.values_of(obj, key)
                text = ", ".join(str(v) for v in values[:2])
                column.label(text=f"{key}: {text[:38]}")
            if len(keys) > 12:
                column.label(text=f"… and {len(keys) - 12} more (Object ▸ Custom Properties)")


class RETE_PT_export(RetePanel, Panel):
    bl_idname = "RETE_PT_export"
    bl_label = "Export"
    bl_options = {"DEFAULT_CLOSED"}

    @classmethod
    def poll(cls, context):
        return engine.available()

    def draw(self, context):
        layout = self.layout
        settings = context.scene.rete
        column = layout.column(align=True)
        column.prop(settings, "export_scope")
        if settings.export_scope == "COLLECTION":
            column.prop_search(settings, "export_collection", bpy.data, "collections", text="")
        column.prop(settings, "export_base")
        column.prop(settings, "export_title")
        column.prop(settings, "export_keep_iris")
        column.prop(settings, "export_path")
        row = layout.row()
        row.scale_y = 1.3
        row.operator(ops.RETE_OT_export_scene.bl_idname, icon="EXPORT")


def _wrap(text: str, width: int):
    """Break a string into label-sized lines — Blender labels do not wrap."""
    if not text:
        return []
    words = str(text).split()
    lines, current = [], ""
    for word in words:
        candidate = f"{current} {word}".strip()
        if len(candidate) > width and current:
            lines.append(current)
            current = word
        else:
            current = candidate
    if current:
        lines.append(current)
    return lines[:6]


CLASSES = (
    RETE_UL_columns,
    RETE_UL_examples,
    RETE_PT_graph,
    RETE_PT_query,
    RETE_PT_columns,
    RETE_PT_build,
    RETE_PT_placement,
    RETE_PT_assets,
    RETE_PT_time,
    RETE_PT_relations,
    RETE_PT_physics,
    RETE_PT_selection,
    RETE_PT_export,
)


def register() -> None:
    for cls in CLASSES:
        bpy.utils.register_class(cls)


def unregister() -> None:
    for cls in reversed(CLASSES):
        bpy.utils.unregister_class(cls)
