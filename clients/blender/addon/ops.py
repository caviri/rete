"""The operators — every action the add-on exposes.

Each one is a thin shell: validate, call into the plain-Python modules, report.
The work itself lives in :mod:`.builder`, :mod:`.export` and friends, so it can
be tested without a UI.
"""

from __future__ import annotations

import os
import traceback
from typing import Dict, List, Optional, Set

import bpy
from bpy.props import BoolProperty, EnumProperty, IntProperty, StringProperty
from bpy.types import Operator

from . import assets, builder, detect, drivers, engine, export, geometry, materials
from . import props as rprops
from . import relations

#: A few published graphs that carry 3D, offered in the UI as a starting point.
CATALOG = [
    (
        "https://data.graphplaza.com/z-anatomy/z-anatomy.rete",
        "Z-Anatomy — the human body",
        "4,884 anatomical structures with mesh geometry, 3D adjacency and per-system .glb assets",
    ),
    (
        "https://data.graphplaza.com/smithsonian3d/smithsonian3d.rete",
        "Smithsonian 3D — 2,199 CC0 models",
        "Crania, fossils, the Apollo command module: every object streams a Draco .glb",
    ),
    (
        "https://data.graphplaza.com/dance/dance.rete",
        "CoMPAS3D — salsa duets",
        "Danced motion as animated skeletons: 3D plus time, straight off the timeline",
    ),
    (
        "https://data.graphplaza.com/bioexplora/bioexplora.rete",
        "Bioexplora — natural history scans",
        "Museu de Ciències Naturals de Barcelona: skull and bone scans as .glb",
    ),
    (
        "https://data.graphplaza.com/geoadmin/geoadmin.rete",
        "geoBoundaries — administrative areas",
        "GeoSPARQL polygons — extrude them into terrain-scale massing",
    ),
]


def _fail(op: Operator, message: str, exc: Optional[BaseException] = None) -> Set[str]:
    if exc is not None:
        traceback.print_exc()
        message = f"{message}: {exc}"
    op.report({"ERROR"}, message)
    return {"CANCELLED"}


def _require_engine(op: Operator) -> bool:
    if engine.available():
        return True
    op.report({"ERROR"}, engine.unavailable_reason())
    return False


class RETE_OT_open_graph(Operator):
    """Open the .rete file and read its self-description"""

    bl_idname = "rete.open_graph"
    bl_label = "Open graph"
    bl_options = {"REGISTER"}

    def execute(self, context):
        settings = context.scene.rete
        if not _require_engine(self):
            return {"CANCELLED"}
        if not settings.source.strip():
            return _fail(self, "no graph source set")

        window = context.window_manager
        window.progress_begin(0, 1)
        try:
            graph = engine.open_graph(settings.source, reopen=True)
            card = graph.card() or {}
            settings.card_title = str(card.get("title", "") or "")
            settings.card_license = str(card.get("license", "") or "")
            counts = []
            for key, label in (("triple_count", "triples"), ("quad_count", "quads"), ("term_count", "terms")):
                if card.get(key):
                    counts.append(f"{int(card[key]):,} {label}")
            settings.card_counts = " · ".join(counts) or f"{graph.quads:,} quads"

            settings.examples.clear()
            for entry in graph.examples():
                item = settings.examples.add()
                item.title = str(entry.get("title", "") or "Example")[:120]
                item.question = str(entry.get("question", "") or "")[:250]
                item.sparql = str(entry.get("sparql", "") or "")
            drivers.set_default_source(settings.source)
            settings.status = f"opened · {len(settings.examples)} example queries"
            self.report({"INFO"}, f"{settings.card_title or settings.source}: {settings.card_counts}")
        except Exception as exc:
            return _fail(self, "could not open the graph", exc)
        finally:
            window.progress_end()
        _update_fetched(settings)
        return {"FINISHED"}


class RETE_OT_pick_file(Operator):
    """Choose a local .rete file"""

    bl_idname = "rete.pick_file"
    bl_label = "Open a .rete file"

    filepath: StringProperty(subtype="FILE_PATH")
    filter_glob: StringProperty(default="*.rete", options={"HIDDEN"})

    def invoke(self, context, event):
        context.window_manager.fileselect_add(self)
        return {"RUNNING_MODAL"}

    def execute(self, context):
        context.scene.rete.source = self.filepath
        return bpy.ops.rete.open_graph()


class RETE_OT_use_catalog(Operator):
    """Use one of the published graphs"""

    bl_idname = "rete.use_catalog"
    bl_label = "Published graphs"

    url: StringProperty()

    def execute(self, context):
        context.scene.rete.source = self.url
        return bpy.ops.rete.open_graph()


class RETE_MT_catalog(bpy.types.Menu):
    bl_idname = "RETE_MT_catalog"
    bl_label = "Published graphs"

    def draw(self, context):
        layout = self.layout
        for url, title, description in CATALOG:
            row = layout.operator(RETE_OT_use_catalog.bl_idname, text=title)
            row.url = url


class RETE_OT_new_query(Operator):
    """Create a text block to hold the query"""

    bl_idname = "rete.new_query"
    bl_label = "New query"

    def execute(self, context):
        settings = context.scene.rete
        text = bpy.data.texts.new("rete query.sparql")
        text.from_string(DEFAULT_QUERY)
        settings.query = text
        return {"FINISHED"}


DEFAULT_QUERY = """\
# Anything this query returns becomes scene content.
# Columns are recognised by what they hold: a .glb URL is an asset, a WKT
# literal is a position, a date is a moment on the timeline.
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?s ?label WHERE {
  ?s rdfs:label ?label .
} LIMIT 200
"""


class RETE_OT_load_example(Operator):
    """Load the dataset's example query into the text block"""

    bl_idname = "rete.load_example"
    bl_label = "Use this example"

    index: IntProperty(default=0)

    def execute(self, context):
        settings = context.scene.rete
        index = self.index if self.index >= 0 else settings.example_index
        if index >= len(settings.examples):
            return _fail(self, "no such example")
        example = settings.examples[index]
        text = settings.query
        if text is None:
            text = bpy.data.texts.new("rete query.sparql")
            settings.query = text
        header = f"# {example.title}\n# {example.question}\n" if example.question else ""
        text.clear()
        text.write(header + example.sparql)
        settings.status = f"loaded example: {example.title}"
        return {"FINISHED"}


def _update_fetched(settings) -> None:
    stats = engine.stats(settings.source)
    if stats:
        megabytes = float(stats.get("bytes", 0)) / (1024 * 1024)
        settings.fetched = f"{megabytes:.2f} MB in {int(stats.get('requests', 0))} requests"


#: The last result, kept out of Blender's property system — it is transient and
#: can be large, and a re-run is one click away.
LAST_RESULT = {"result": None}


class RETE_OT_run_query(Operator):
    """Run the query and inspect what came back"""

    bl_idname = "rete.run_query"
    bl_label = "Run query"

    def execute(self, context):
        settings = context.scene.rete
        if not _require_engine(self):
            return {"CANCELLED"}
        sparql = settings.query_text().strip()
        if not sparql:
            return _fail(self, "the query is empty — create a query text block first")

        window = context.window_manager
        window.progress_begin(0, 1)
        try:
            result = engine.select(settings.source, sparql, reason=settings.reason)
        except Exception as exc:
            window.progress_end()
            return _fail(self, "query failed", exc)
        window.progress_end()

        LAST_RESULT["result"] = result
        settings.row_count = len(result)
        roles = detect.classify_result(result)

        settings.columns.clear()
        for var in result.vars:
            column = settings.columns.add()
            column.name = var
            column.detected = roles.get(var, detect.TEXT)
            first = next((c for c in result.column(var) if c is not None), None)
            column.sample = (first.value[:70] if first else "—")

        settings.status = f"{len(result):,} rows · {len(result.vars)} columns"
        _update_fetched(settings)
        _suggest_defaults(settings, result, roles)
        self.report({"INFO"}, settings.status)
        return {"FINISHED"}


def _suggest_defaults(settings, result, roles: Dict[str, str]) -> None:
    """Preset the switches the result obviously wants.

    Guessing here is safe — everything remains user-editable, and a result full
    of dates that leaves the timeline off is a worse first impression than one
    that switched it on.
    """
    binding = detect.resolve(result, roles, settings.overrides())
    if binding.time and settings.time_mode == "NONE":
        settings.time_mode = "APPEAR"
        times = [
            c.value for c in result.column(binding.time) if c is not None
        ][:1]
        settings.time_span = f"from {times[0]}" if times else ""
    if binding.asset and not settings.import_assets:
        settings.import_assets = True
    if binding.geometries:
        coords = []
        for cell in result.column(binding.geometries[0])[:200]:
            if cell is None:
                continue
            parsed = geometry.parse(cell.value)
            if parsed:
                coords.extend(parsed.coords)
        if coords and geometry.looks_geographic(coords):
            settings.status += " · geographic coordinates detected"
    if not settings.material_var and binding.numbers:
        settings.material_var = binding.numbers[0]


class RETE_OT_build_scene(Operator):
    """Build the scene from the current result"""

    bl_idname = "rete.build_scene"
    bl_label = "Build scene"
    bl_options = {"REGISTER", "UNDO"}

    def execute(self, context):
        settings = context.scene.rete
        if not _require_engine(self):
            return {"CANCELLED"}
        result = LAST_RESULT.get("result")
        if result is None:
            status = bpy.ops.rete.run_query()
            if "FINISHED" not in status:
                return {"CANCELLED"}
            result = LAST_RESULT.get("result")
        if result is None:
            return _fail(self, "no result to build from")

        window = context.window_manager
        window.progress_begin(0, 1)
        try:
            report = builder.build(result, settings.to_build_settings(), context)
        except Exception as exc:
            window.progress_end()
            return _fail(self, "build failed", exc)
        window.progress_end()

        for warning in report.warnings:
            self.report({"WARNING"}, warning)
        settings.status = report.summary()
        _update_fetched(settings)
        drivers.set_default_source(settings.source)
        self.report({"INFO"}, f"built {report.summary()}")
        return {"FINISHED"}


class RETE_OT_discover_predicates(Operator):
    """List the predicates the imported entities actually use"""

    bl_idname = "rete.discover_predicates"
    bl_label = "Find predicates"

    def execute(self, context):
        settings = context.scene.rete
        if not _require_engine(self):
            return {"CANCELLED"}
        iris = list(rprops.objects_with_iri(context).keys())
        if not iris:
            result = LAST_RESULT.get("result")
            if result is not None:
                roles = detect.classify_result(result)
                binding = detect.resolve(result, roles, settings.overrides())
                if binding.entity:
                    iris = [c.value for c in result.column(binding.entity) if c is not None]
        if not iris:
            return _fail(self, "nothing in the scene carries a graph IRI yet — build first")

        try:
            found = engine.predicates_of(settings.source, iris)
        except Exception as exc:
            return _fail(self, "could not list predicates", exc)

        settings.predicates.clear()
        for predicate, count in found:
            item = settings.predicates.add()
            item.name = predicate
            item.count = count
        settings.status = f"{len(found)} predicates in use"
        self.report({"INFO"}, settings.status)
        return {"FINISHED"}


class RETE_OT_expand_selection(Operator):
    """Import the graph neighbours of the selected objects"""

    bl_idname = "rete.expand_selection"
    bl_label = "Expand neighbours"
    bl_options = {"REGISTER", "UNDO"}

    predicate: StringProperty(name="Predicate", description="Leave empty to follow every relation")
    inverse: BoolProperty(name="Follow backwards", default=False)
    limit: IntProperty(name="Limit", default=200, min=1, max=5000)

    def execute(self, context):
        settings = context.scene.rete
        if not _require_engine(self):
            return {"CANCELLED"}
        selected = [o for o in context.selected_objects if o.get(rprops.IRI)]
        if not selected:
            return _fail(self, "select at least one object that came from the graph")

        known = rprops.objects_with_iri(context)
        seeds = [str(o[rprops.IRI]) for o in selected]
        collection = builder.get_collection(settings.collection_name or "rete", context.scene)

        try:
            neighbours = self._neighbours(settings.source, seeds)
        except Exception as exc:
            return _fail(self, "neighbour query failed", exc)

        new_iris = [i for i in neighbours if i not in known][: self.limit]
        if not new_iris:
            self.report({"INFO"}, "no new neighbours found")
            return {"FINISHED"}

        labels = engine.labels_for(settings.source, new_iris)
        created: Dict[str, "bpy.types.Object"] = {}
        anchor = selected[0].matrix_world.translation
        for index, iri in enumerate(new_iris):
            name = labels.get(iri) or rprops.local_name(iri)
            obj = builder._primitive(settings.point_style, settings.point_size, name[:60])
            if obj is None:
                continue
            collection.objects.link(obj)
            offset = builder._layout_position(index, len(new_iris), settings.to_build_settings())
            obj.location = (anchor[0] + offset[0], anchor[1] + offset[1], anchor[2] + offset[2])
            rprops.stamp_identity(obj, iri, settings.source)
            materials.assign(obj, materials.solid("expanded", materials.color_for_key(iri)))
            created[iri] = obj

        if settings.deep_properties and created:
            try:
                for iri, statements in engine.describe_many(settings.source, list(created)).items():
                    if statements and iri in created:
                        rprops.stamp(created[iri], statements)
            except Exception:
                pass

        # Draw what connects the new objects to the old ones.
        everything = dict(known)
        everything.update(created)
        if self.predicate:
            edges = relations.fetch_edges(settings.source, everything, self.predicate, inverse=self.inverse)
            relations.build_edge_mesh(edges, f"edges:{rprops.local_name(self.predicate)}", collection)

        _update_fetched(settings)
        settings.status = f"expanded to {len(created)} neighbours"
        self.report({"INFO"}, settings.status)
        return {"FINISHED"}

    def _neighbours(self, source: str, seeds: List[str]) -> List[str]:
        out: List[str] = []
        seen: Set[str] = set()
        if self.predicate:
            pairs = engine.pairs_by_predicate(source, seeds, self.predicate, inverse=self.inverse)
            candidates = [cell for _, cell in pairs]
        else:
            described = engine.describe_many(source, seeds)
            candidates = [cell for statements in described.values() for _, cell in statements]
        for cell in candidates:
            if cell.is_iri and cell.value not in seen:
                seen.add(cell.value)
                out.append(cell.value)
        return out

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self)


class RETE_OT_select_by_query(Operator):
    """Select the objects a query returns"""

    bl_idname = "rete.select_by_query"
    bl_label = "Select by query"
    bl_options = {"REGISTER", "UNDO"}

    extend: BoolProperty(name="Extend selection", default=False)

    def execute(self, context):
        settings = context.scene.rete
        if not _require_engine(self):
            return {"CANCELLED"}
        sparql = settings.query_text().strip()
        if not sparql:
            return _fail(self, "the query is empty")
        try:
            result = engine.select(settings.source, sparql, reason=settings.reason)
        except Exception as exc:
            return _fail(self, "query failed", exc)

        wanted: Set[str] = set()
        for row in result.rows:
            for cell in row.values():
                if cell is not None and cell.is_iri:
                    wanted.add(cell.value)

        if not self.extend:
            for obj in context.selected_objects:
                obj.select_set(False)
        hits = 0
        for iri, obj in rprops.objects_with_iri(context).items():
            if iri in wanted:
                obj.select_set(True)
                context.view_layer.objects.active = obj
                hits += 1
        settings.status = f"selected {hits} objects"
        self.report({"INFO"}, settings.status)
        return {"FINISHED"}


class RETE_OT_describe_active(Operator):
    """Fetch every statement about the active object's entity"""

    bl_idname = "rete.describe_active"
    bl_label = "Refresh properties"

    def execute(self, context):
        obj = context.active_object
        if obj is None or not obj.get(rprops.IRI):
            return _fail(self, "the active object has no graph IRI")
        source = rprops.source_of(obj) or context.scene.rete.source
        try:
            described = engine.describe_many(source, [str(obj[rprops.IRI])])
        except Exception as exc:
            return _fail(self, "describe failed", exc)
        written = rprops.stamp(obj, described.get(str(obj[rprops.IRI]), []))
        self.report({"INFO"}, f"{written} properties inherited")
        return {"FINISHED"}


class RETE_OT_export_scene(Operator):
    """Write the scene out as a queryable .rete file"""

    bl_idname = "rete.export_scene"
    bl_label = "Export .rete"

    def execute(self, context):
        settings = context.scene.rete
        if not _require_engine(self):
            return {"CANCELLED"}
        objects = export.collect_objects(
            context, scope=settings.export_scope, collection_name=settings.export_collection
        )
        if not objects:
            return _fail(self, "nothing to export in that scope")

        path = bpy.path.abspath(settings.export_path)
        if not path.endswith(".rete"):
            path += ".rete"
        directory = os.path.dirname(path)
        if directory and not os.path.isdir(directory):
            return _fail(self, f"no such directory: {directory}")

        window = context.window_manager
        window.progress_begin(0, 1)
        try:
            ntriples, count = export.scene_to_ntriples(
                objects,
                base=settings.export_base,
                scene_name=context.scene.name,
                keep_iris=settings.export_keep_iris,
            )
            stats = export.build_rete(ntriples, path, title=settings.export_title)
        except Exception as exc:
            window.progress_end()
            return _fail(self, "export failed", exc)
        window.progress_end()

        size = os.path.getsize(path) / 1024.0
        settings.status = f"exported {count} objects · {stats.get('statements', '?')} statements · {size:.0f} KB"
        self.report({"INFO"}, f"{path} — {settings.status}")
        return {"FINISHED"}


class RETE_OT_clear_cache(Operator):
    """Delete the downloaded assets and the driver cache"""

    bl_idname = "rete.clear_cache"
    bl_label = "Clear cache"

    def execute(self, context):
        files = assets.clear_cache()
        cached = drivers.clear_cache()
        engine.close_all()
        self.report({"INFO"}, f"removed {files} cached files, {cached} driver values")
        return {"FINISHED"}


CLASSES = (
    RETE_OT_open_graph,
    RETE_OT_pick_file,
    RETE_OT_use_catalog,
    RETE_MT_catalog,
    RETE_OT_new_query,
    RETE_OT_load_example,
    RETE_OT_run_query,
    RETE_OT_build_scene,
    RETE_OT_discover_predicates,
    RETE_OT_expand_selection,
    RETE_OT_select_by_query,
    RETE_OT_describe_active,
    RETE_OT_export_scene,
    RETE_OT_clear_cache,
)


def register() -> None:
    for cls in CLASSES:
        bpy.utils.register_class(cls)


def unregister() -> None:
    for cls in reversed(CLASSES):
        bpy.utils.unregister_class(cls)
