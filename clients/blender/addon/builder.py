"""Turning a query result into a Blender scene.

The pipeline, in order:

1. resolve which column plays which role,
2. parse geometry and work out one shared placement transform,
3. create an object per entity — an imported asset, a mesh built from the
   geometry literal, or a laid-out placeholder,
4. inherit the RDF properties onto it,
5. then the optional passes: materials, timeline, relations, physics.

Everything here is plain Python against ``bpy``: no operator, no UI state, no
scene assumptions. That is what lets the whole thing run headless in a test.
"""

from __future__ import annotations

import math
from typing import Dict, List, Optional, Sequence, Set, Tuple

import bpy

from . import assets, attributes, detect, engine, geometry, materials, physics
from . import props as rprops
from . import relations, timeline

POINT_STYLES = (
    ("SPHERE", "Sphere", "A small sphere — visible, renderable, and physics-ready"),
    ("CUBE", "Cube", "A small cube"),
    ("EMPTY", "Empty", "A non-renderable marker — cheapest for large results"),
    ("NONE", "None", "No placeholder; only rows with an asset become objects"),
)

LAYOUTS = (
    ("AUTO", "Auto", "Use the geometry if there is any, otherwise a grid"),
    ("GEOMETRY", "Geometry", "Position from the geometry column only"),
    ("GRID", "Grid", "Lay the rows out in a regular grid"),
    ("CIRCLE", "Circle", "Arrange the rows around a circle"),
    ("SCATTER", "Scatter plot", "Position from up to three numeric columns"),
    ("NONE", "Stacked at origin", "Leave everything at the origin"),
)

SCALE_MODES = (
    ("FIT", "Fit to size", "Scale the whole result to fit a given size in metres"),
    ("MM", "Millimetres", "Source coordinates are millimetres"),
    ("CM", "Centimetres", "Source coordinates are centimetres"),
    ("M", "Metres", "Source coordinates are metres — no scaling"),
    ("KM", "Kilometres", "Source coordinates are kilometres"),
    ("CUSTOM", "Custom", "Multiply coordinates by a factor you choose"),
)

UNIT_SCALE = {"MM": 0.001, "CM": 0.01, "M": 1.0, "KM": 1000.0}

#: Asset URL extensions whose files carry their own real-world coordinates, so
#: an import keeps its transform rather than being laid out.
_WORLD_PLACED_EXT = frozenset((".ifc", ".ifczip", ".ifcxml", ".dxf"))

TIME_MODES = (
    ("NONE", "None", "Ignore the time column"),
    ("APPEAR", "Appear", "Objects appear (and disappear) at their moment"),
    ("GROW", "Grow in", "Objects scale up as their moment arrives"),
    ("PATH", "Motion path", "Rows sharing an entity become a keyframed trajectory"),
    ("RETIME", "Retime assets", "Place each asset's own animation at its moment"),
)


class Settings:
    """Everything the builder needs, independent of Blender's UI types.

    The add-on's PropertyGroup converts itself into one of these, so the builder
    is callable from a script or a test with a plain object.
    """

    def __init__(self, **kw):
        self.source: str = kw.get("source", "")
        self.query: str = kw.get("query", "")
        self.collection_name: str = kw.get("collection_name", "rete")
        self.limit: int = kw.get("limit", 5000)

        self.point_style: str = kw.get("point_style", "SPHERE")
        self.point_size: float = kw.get("point_size", 0.05)
        self.layout: str = kw.get("layout", "AUTO")
        self.layout_spacing: float = kw.get("layout_spacing", 1.0)
        self.scatter_vars: Tuple[str, str, str] = kw.get("scatter_vars", ("", "", ""))

        self.scale_mode: str = kw.get("scale_mode", "FIT")
        self.fit_size: float = kw.get("fit_size", 10.0)
        self.custom_scale: float = kw.get("custom_scale", 1.0)
        self.axis_up: str = kw.get("axis_up", "Z")
        self.flip_x: bool = kw.get("flip_x", False)
        self.recentre: bool = kw.get("recentre", True)
        self.extrude: float = kw.get("extrude", 0.0)

        self.import_assets: bool = kw.get("import_assets", True)
        self.asset_scale: float = kw.get("asset_scale", 1.0)
        self.place_assets: str = kw.get("place_assets", "AUTO")
        self.max_assets: int = kw.get("max_assets", 400)

        self.deep_properties: bool = kw.get("deep_properties", True)
        self.reason: bool = kw.get("reason", False)

        self.material_mode: str = kw.get("material_mode", "AUTO")
        self.material_var: str = kw.get("material_var", "")
        self.texture_size: int = kw.get("texture_size", 2048)

        # Media (images, video) and maps (PMTiles).
        self.image_mode: str = kw.get("image_mode", "MATERIAL")  # MATERIAL/PLANE/WORLD
        self.media_height: float = kw.get("media_height", 1.0)
        self.map_mode: str = kw.get("map_mode", "AUTO")           # AUTO/VECTOR/RASTER
        self.map_zoom: int = kw.get("map_zoom", -1)               # -1 = auto
        self.map_tiles: int = kw.get("map_tiles", 40)
        self.map_extrude: float = kw.get("map_extrude", 0.0)
        self.splat_points: int = kw.get("splat_points", 200_000)  # preview cap

        self.time_mode: str = kw.get("time_mode", "NONE")
        self.frame_start: int = kw.get("frame_start", 1)
        self.frame_end: int = kw.get("frame_end", 250)

        self.relation_mode: str = kw.get("relation_mode", "NONE")
        self.relation_predicate: str = kw.get("relation_predicate", "")
        self.relation_inverse: bool = kw.get("relation_inverse", False)

        self.physics_mode: str = kw.get("physics_mode", "NONE")
        self.physics_shape: str = kw.get("physics_shape", "CONVEX_HULL")
        self.physics_mass_var: str = kw.get("physics_mass_var", "")
        self.constraint_predicate: str = kw.get("constraint_predicate", "")
        self.constraint_type: str = kw.get("constraint_type", "FIXED")

        self.point_cloud: bool = kw.get("point_cloud", False)
        self.overrides: Dict[str, str] = kw.get("overrides", {})


class Report:
    """What a build did, for the status line and the test assertions."""

    def __init__(self):
        self.objects = 0
        self.assets = 0
        self.properties = 0
        self.materials = 0
        self.keyframed = 0
        self.relations = 0
        self.bodies = 0
        self.constraints = 0
        self.media = 0
        self.map_layers = 0
        self.warnings: List[str] = []
        self.collection: Optional["bpy.types.Collection"] = None

    def warn(self, message: str) -> None:
        if message and message not in self.warnings:
            self.warnings.append(message)

    def summary(self) -> str:
        bits = [f"{self.objects} objects"]
        if self.assets:
            bits.append(f"{self.assets} assets")
        if self.properties:
            bits.append(f"{self.properties} properties")
        if self.keyframed:
            bits.append(f"{self.keyframed} animated")
        if self.relations:
            bits.append(f"{self.relations} relations")
        if self.constraints:
            bits.append(f"{self.constraints} constraints")
        if self.media:
            bits.append(f"{self.media} media")
        if self.map_layers:
            bits.append(f"{self.map_layers} map layers")
        return ", ".join(bits)


# ------------------------------------------------------------------- helpers


def get_collection(name: str, scene: "bpy.types.Scene") -> "bpy.types.Collection":
    coll = bpy.data.collections.get(name)
    if coll is None:
        coll = bpy.data.collections.new(name)
        scene.collection.children.link(coll)
    elif name not in {c.name for c in scene.collection.children_recursive}:
        try:
            scene.collection.children.link(coll)
        except RuntimeError:
            pass
    return coll


def _object_name(row: Dict, binding: detect.Binding, iri: str, index: int) -> str:
    label_var = binding.label
    if label_var and row.get(label_var) is not None:
        text = row[label_var].value.strip()
        if text:
            return text[:60]
    if iri:
        return rprops.local_name(iri)[:60] or f"row {index}"
    return f"row {index}"


def _mesh_from_geometry(
    geom: geometry.Geometry,
    placement: geometry.Placement,
    name: str,
    *,
    extrude: float = 0.0,
) -> Optional["bpy.types.Mesh"]:
    """Build real geometry for a line, an area or a multipoint literal."""
    mesh = bpy.data.meshes.new(name)
    verts: List[Tuple[float, float, float]] = []
    edges: List[Tuple[int, int]] = []
    faces: List[List[int]] = []

    origin = placement.apply(geom.centroid)

    for ring in geom.rings:
        pts = [placement.apply(c) for c in ring]
        # Local coordinates: the object carries the world position.
        pts = [(p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]) for p in pts]
        if geom.kind == geometry.AREA and len(pts) >= 3:
            if pts[0] == pts[-1]:
                pts = pts[:-1]
            if len(pts) < 3:
                continue
            base = len(verts)
            verts.extend(pts)
            faces.append(list(range(base, base + len(pts))))
        elif geom.kind == geometry.LINE and len(pts) >= 2:
            base = len(verts)
            verts.extend(pts)
            edges.extend((base + i, base + i + 1) for i in range(len(pts) - 1))
        else:
            verts.extend(pts)

    if not verts:
        bpy.data.meshes.remove(mesh)
        return None
    mesh.from_pydata(verts, edges, faces)
    mesh.update()

    if extrude and faces:
        _extrude(mesh, extrude)
    return mesh


def _extrude(mesh: "bpy.types.Mesh", height: float) -> None:
    """Give a flat polygon thickness — footprints become massing models."""
    import bmesh

    bm = bmesh.new()
    bm.from_mesh(mesh)
    faces = list(bm.faces)
    if faces:
        result = bmesh.ops.extrude_face_region(bm, geom=faces)
        moved = [v for v in result["geom"] if isinstance(v, bmesh.types.BMVert)]
        bmesh.ops.translate(bm, verts=moved, vec=(0.0, 0.0, height))
    bm.to_mesh(mesh)
    bm.free()


def _primitive(style: str, size: float, name: str) -> Optional["bpy.types.Object"]:
    if style == "NONE":
        return None
    if style == "EMPTY":
        obj = bpy.data.objects.new(name, None)
        obj.empty_display_type = "SPHERE"
        obj.empty_display_size = size
        return obj
    mesh = _primitive_mesh(style, size)
    return bpy.data.objects.new(name, mesh)


_PRIMITIVE_CACHE: Dict[Tuple[str, float], "bpy.types.Mesh"] = {}


def _primitive_mesh(style: str, size: float) -> "bpy.types.Mesh":
    """One shared mesh per (style, size) — thousands of markers, one datablock."""
    key = (style, round(size, 6))
    cached = _PRIMITIVE_CACHE.get(key)
    if cached is not None and cached.users >= 0:
        try:
            _ = cached.name
            return cached
        except ReferenceError:
            pass

    import bmesh

    mesh = bpy.data.meshes.new(f"rete:{style.lower()}:{size:g}")
    bm = bmesh.new()
    if style == "CUBE":
        bmesh.ops.create_cube(bm, size=size * 2.0)
    else:
        bmesh.ops.create_icosphere(bm, subdivisions=2, radius=size)
    bm.to_mesh(mesh)
    bm.free()
    _PRIMITIVE_CACHE[key] = mesh
    return mesh


def _layout_position(index: int, total: int, settings: Settings) -> Tuple[float, float, float]:
    spacing = settings.layout_spacing
    if settings.layout == "CIRCLE":
        radius = max(1.0, spacing * total / (2.0 * math.pi))
        angle = 2.0 * math.pi * index / max(1, total)
        return (radius * math.cos(angle), radius * math.sin(angle), 0.0)
    columns = max(1, int(math.ceil(math.sqrt(max(1, total)))))
    row, col = divmod(index, columns)
    offset = (columns - 1) * spacing / 2.0
    return (col * spacing - offset, -row * spacing + offset, 0.0)


# --------------------------------------------------------------------- build


def build(result, settings: Settings, context=None) -> Report:
    """Build a scene from a query result. The one entry point."""
    context = context or bpy.context
    scene = context.scene
    report = Report()

    rows = result.rows[: settings.limit]
    if not rows:
        report.warn("the query returned no rows")
        return report
    if len(result.rows) > len(rows):
        report.warn(f"showing the first {len(rows)} of {len(result.rows)} rows (raise the limit)")

    roles = detect.classify_result(result)
    binding = detect.resolve(result, roles, settings.overrides)
    collection = get_collection(settings.collection_name, scene)
    report.collection = collection

    # -- geometry and the shared placement -------------------------------
    geom_vars = binding.geometries
    geoms: List[Optional[geometry.Geometry]] = []
    boxes: List[Optional[geometry.Geometry]] = []
    geom_datatypes: Set[str] = set()
    for row in rows:
        primary = None
        box = None
        for var in geom_vars:
            cell = row.get(var)
            if cell is None:
                continue
            parsed = geometry.parse(cell.value)
            if parsed is None:
                continue
            if cell.datatype:
                geom_datatypes.add(cell.datatype)
            if parsed.kind == geometry.BOX:
                box = box or parsed
            elif primary is None:
                primary = parsed
        geoms.append(primary or box)
        boxes.append(box)

    # A row's position comes from its geometry, or from numeric axis columns
    # when the dataset publishes coordinates as plain x/y/z instead of WKT.
    axis_vars = [v for v in binding.xyz if v]
    positions: List[Optional[Tuple[float, float, float]]] = []
    for index, row in enumerate(rows):
        if geoms[index] is not None:
            positions.append(geoms[index].centroid)
        elif axis_vars:
            positions.append(_axis_coords(row, binding))
        else:
            positions.append(None)

    placement = _placement(
        [p for p in positions if p is not None],
        settings,
        from_wkt=any(g is not None for g in geoms),
        lonlat_named=any(
            v.lower().lstrip("?").startswith(("lon", "lat")) for v in axis_vars
        ),
        datatypes=sorted(geom_datatypes),
    )

    # -- one object per entity -------------------------------------------
    by_iri: Dict[str, "bpy.types.Object"] = {}
    entity_var = binding.entity
    asset_var = binding.asset if settings.import_assets else None
    node_var = binding.mesh_node
    order: List[str] = []
    paths: Dict[str, List[Tuple[float, Sequence[float]]]] = {}

    times = timeline.collect(rows, binding.time)
    ends = timeline.collect(rows, binding.time_end)
    mapper = None
    if settings.time_mode != "NONE":
        axis = [t for t in times + ends if t is not None]
        if axis:
            mapper = timeline.Mapper(axis, settings.frame_start, settings.frame_end)
            timeline.set_scene_range(scene, settings.frame_start, settings.frame_end)
        else:
            report.warn("no usable time values — the timeline pass was skipped")

    if settings.point_cloud:
        return _build_point_cloud(rows, result, binding, roles, placement, positions, settings, report)

    # In motion-path mode the object is the thing that moves, which is rarely
    # the row's own subject: a trajectory dataset gives every sample its own
    # IRI and names the moving object in a second column.
    identity_var = entity_var
    if settings.time_mode == "PATH":
        identity_var = _path_group_var(rows, binding) or entity_var

    video_var = binding.video
    image_var = binding.image
    splat_var = binding.splat
    max_video_frames = 0

    asset_budget = settings.max_assets
    for index, row in enumerate(rows):
        iri = row[identity_var].value if identity_var and row.get(identity_var) is not None else ""
        key = iri or f"__row{index}"

        # A repeated identity with a time column is a trajectory, not a second
        # object: the row contributes a keyframe instead.
        if key in by_iri:
            if settings.time_mode == "PATH" and mapper is not None and positions[index] is not None:
                frame = mapper.frame(times[index])
                if frame is not None:
                    paths.setdefault(key, []).append((frame, placement.apply(positions[index])))
            continue

        name = _object_name(row, binding, iri, index)
        obj = None
        geom = geoms[index]

        asset_url = row[asset_var].value if asset_var and row.get(asset_var) is not None else ""
        node_name = row[node_var].value if node_var and row.get(node_var) is not None else ""

        # A splat is its own path — either an explicit splat column, or an asset
        # column that turns out to be a splat .ply on inspection.
        splat_url = row[splat_var].value if splat_var and row.get(splat_var) is not None else ""
        if not splat_url and asset_url and assets.is_splat_asset(asset_url):
            splat_url, asset_url = asset_url, ""

        keep_transform = False
        if splat_url and asset_budget > 0:
            obj, note = _splat_object(splat_url, name, collection, settings)
            if obj is None:
                report.warn(note)
            else:
                report.assets += 1
                asset_budget -= 1
                if note:
                    report.warn(note)
        elif splat_url:
            report.warn(f"asset limit ({settings.max_assets}) reached — remaining rows are markers")

        if obj is None and asset_url and asset_budget > 0:
            # A CAD/BIM asset carries real coordinates; so does any asset paired
            # with a geometry column while we lay out by geometry. Either way it
            # should stay where it lands rather than be re-placed.
            world_placed = (
                detect.url_extension(asset_url) in _WORLD_PLACED_EXT
                or (positions[index] is not None and settings.layout in ("AUTO", "GEOMETRY"))
            )
            obj, note, keep_transform = _asset_object(
                asset_url, node_name, name, collection, settings,
                world_placed=world_placed,
            )
            if obj is None:
                report.warn(note)
            else:
                report.assets += 1
                asset_budget -= 1
        elif asset_url and obj is None:
            report.warn(f"asset limit ({settings.max_assets}) reached — remaining rows are markers")

        # Video and image-plane rows become upright screens at the row's spot.
        if obj is None and video_var and row.get(video_var) is not None:
            from . import media

            made = media.video_plane(
                row[video_var].value, name, collection,
                height=settings.media_height, frame_start=settings.frame_start,
            )
            if made is not None:
                obj, frames = made
                report.media += 1
                max_video_frames = max(max_video_frames, frames)
            else:
                report.warn(f"could not load video {row[video_var].value}")
        if (
            obj is None
            and settings.image_mode == "PLANE"
            and image_var
            and row.get(image_var) is not None
        ):
            from . import media

            obj = media.image_plane(
                row[image_var].value, name, collection,
                height=settings.media_height, max_pixels=settings.texture_size,
            )
            if obj is not None:
                report.media += 1

        # A row that only carries a map URL (built separately, scene-wide) should
        # not also leave a stray marker behind.
        if obj is None and geom is None and not iri:
            if any(row.get(m) is not None for m in binding.maps):
                continue

        if obj is None:
            obj = _geometry_object(geom, placement, name, settings)
            keep_transform = False
        if obj is None:
            continue
        if obj.name not in collection.objects:
            collection.objects.link(obj)

        _place(
            obj, positions[index], geom, boxes[index], placement, settings,
            index, len(rows), row, binding, keep_transform=keep_transform,
        )

        if iri:
            rprops.stamp_identity(obj, iri, settings.source, settings.query)
            by_iri[iri] = obj
            order.append(iri)
        report.properties += rprops.stamp_row(
            obj, row, skip={v for v in (identity_var,) if v}
        )
        report.objects += 1

        if settings.time_mode == "PATH" and mapper is not None:
            frame = mapper.frame(times[index])
            if frame is not None:
                paths.setdefault(key, []).append((frame, tuple(obj.location)))

    # -- inherit every statement -----------------------------------------
    if settings.deep_properties and by_iri:
        try:
            described = engine.describe_many(
                settings.source, order, reason=settings.reason
            )
            for iri, statements in described.items():
                obj = by_iri.get(iri)
                if obj is not None and statements:
                    report.properties += rprops.stamp(obj, statements)
        except Exception as exc:
            report.warn(f"property inheritance failed: {exc}")

    # -- the optional passes ----------------------------------------------
    report.materials = _apply_materials(rows, by_iri, binding, identity_var, settings, report)

    if mapper is not None:
        report.keyframed = _apply_time(
            rows, by_iri, identity_var, times, ends, mapper, paths, settings
        )

    if settings.relation_mode != "NONE" and settings.relation_predicate and by_iri:
        try:
            made, note = relations.apply(
                settings.relation_mode,
                settings.source,
                by_iri,
                settings.relation_predicate,
                collection,
                inverse=settings.relation_inverse,
            )
            report.relations = made
            if made == 0 and note:
                report.warn(note)
        except Exception as exc:
            report.warn(f"relation pass failed: {exc}")

    if settings.physics_mode != "NONE":
        _apply_physics(by_iri, settings, report)

    # -- media and maps ---------------------------------------------------
    if max_video_frames > 0:
        # Make sure the timeline is long enough to play the longest clip.
        scene.frame_end = max(scene.frame_end, settings.frame_start + max_video_frames)

    if settings.image_mode == "WORLD" and image_var:
        from . import media

        first = next((row[image_var].value for row in rows if row.get(image_var) is not None), "")
        if first and media.set_world_panorama(first):
            report.media += 1
        elif first:
            report.warn("could not set the world panorama")

    if binding.maps:
        _apply_maps(rows, binding, placement, positions, collection, settings, report)

    return report


def _apply_maps(rows, binding, placement, positions, collection, settings, report) -> None:
    """Build every distinct PMTiles map the result points at."""
    from . import tiles

    # The map projects lon/lat through a geographic placement. Reuse the scene's
    # if it is already geographic (so the map aligns with the points on it);
    # otherwise build one centred on the map's extent.
    coords = [p for p in positions if p is not None]
    bbox = None
    if placement.geographic and coords:
        lons = [c[0] for c in coords]
        lats = [c[1] for c in coords]
        pad = 0.5
        bbox = (min(lons) - pad, min(lats) - pad, max(lons) + pad, max(lats) + pad)

    urls: List[str] = []
    for var in binding.maps:
        for row in rows:
            cell = row.get(var)
            if cell is not None and cell.value not in urls:
                urls.append(cell.value)

    map_placement = placement if placement.geographic else _map_placement(bbox, settings)
    for url in urls[:8]:
        try:
            objects, note = tiles.build_map(
                url,
                placement=map_placement,
                bbox=bbox,
                zoom=settings.map_zoom,
                max_tiles=settings.map_tiles,
                extrude=settings.map_extrude,
                collection=collection,
                name=_map_name(url),
            )
        except Exception as exc:
            report.warn(f"map {url.rsplit('/', 1)[-1]} failed: {exc}")
            continue
        for obj in objects:
            _colour_map_layer(obj)
        report.objects += len(objects)
        report.map_layers += len(objects)
        if note:
            report.warn(note)


def _map_placement(bbox, settings: Settings) -> geometry.Placement:
    """A geographic placement for a stand-alone map (no other geometry)."""
    from . import tiles

    box = bbox or tiles.WORLD_BBOX
    ref_lon = (box[0] + box[2]) / 2.0
    ref_lat = (box[1] + box[3]) / 2.0
    placement = geometry.Placement(
        scale=1.0, axis_up=settings.axis_up, flip_x=settings.flip_x,
        geographic=True, ref_lon=ref_lon, ref_lat=ref_lat,
    )
    # Fit the projected extent to the requested size.
    corners = [
        (box[0], box[1], 0.0), (box[2], box[1], 0.0),
        (box[2], box[3], 0.0), (box[0], box[3], 0.0),
    ]
    if settings.recentre:
        placement.offset = geometry.centre_of(corners, placement)
    if settings.scale_mode == "FIT":
        placement.scale = geometry.fit_scale(corners, settings.fit_size, placement)
    elif settings.scale_mode == "CUSTOM":
        placement.scale = settings.custom_scale
    else:
        placement.scale = UNIT_SCALE.get(settings.scale_mode, 1.0)
    return placement


def _map_name(url: str) -> str:
    base = url.rsplit("/", 1)[-1]
    return base[:-8] if base.endswith(".pmtiles") else (base or "map")


def _colour_map_layer(obj: "bpy.types.Object") -> None:
    layer = str(obj.get("rete:mapLayer", "") or obj.name)
    materials.assign(obj, materials.solid(f"map:{layer}", materials.color_for_key(layer)))


def _placement(
    coords: Sequence[Tuple[float, float, float]],
    settings: Settings,
    *,
    from_wkt: bool = False,
    lonlat_named: bool = False,
    datatypes: Sequence[str] = (),
) -> geometry.Placement:
    """One transform for the whole result: projection, scale, recentre.

    Takes every coordinate the result produced — from geometry literals and from
    numeric axis columns alike — so a dataset that publishes positions as plain
    x/y columns gets the same fitting and recentring as one that publishes WKT.
    """
    geographic = geometry.is_geographic(
        coords, from_wkt=from_wkt, lonlat_named=lonlat_named, datatypes=datatypes
    )
    ref_lon, ref_lat = geometry.reference_lonlat(coords) if geographic else (0.0, 0.0)

    placement = geometry.Placement(
        scale=1.0,
        axis_up=settings.axis_up,
        flip_x=settings.flip_x,
        geographic=geographic,
        ref_lon=ref_lon,
        ref_lat=ref_lat,
    )
    if settings.recentre and coords:
        placement.offset = geometry.centre_of(coords, placement)

    if settings.scale_mode == "FIT":
        placement.scale = geometry.fit_scale(coords, settings.fit_size, placement) if coords else 1.0
    elif settings.scale_mode == "CUSTOM":
        placement.scale = settings.custom_scale
    else:
        placement.scale = UNIT_SCALE.get(settings.scale_mode, 1.0)
    return placement


def _asset_object(
    url: str,
    node: str,
    name: str,
    collection: "bpy.types.Collection",
    settings: Settings,
    *,
    world_placed: bool = False,
) -> Tuple[Optional["bpy.types.Object"], str, bool]:
    """An object for one row's asset — a node of a shared file, or the whole file.

    Returns ``(object, note, keep_transform)``. Isolated nodes keep the transform
    they have inside the asset, because that *is* their position: the anatomy
    graph's nine body-system files place every structure correctly already.
    ``world_placed`` marks a whole-file asset that already carries real
    coordinates — an IFC/CAD model, or a glTF exported at building/geographic
    coordinates — so it stays where it lands instead of being laid out.
    """
    try:
        if node:
            templates, exact = assets.find_nodes(url, node)
            if not templates:
                return (None, f"node {node!r} not found in {url.rsplit('/', 1)[-1]}", False)
            note = "" if exact else (
                f"node {node!r} matched {len(templates)} related part(s) by name — "
                f"the file names its pieces differently"
            )
            if len(templates) == 1:
                obj = assets.instance(templates[0], name, collection)
                obj.parent = None
                obj.matrix_world = templates[0].matrix_world.copy()
                return (obj, note, True)
            # Several pieces make up the structure: group them under one object.
            obj = bpy.data.objects.new(name, None)
            obj.empty_display_size = 0.05
            collection.objects.link(obj)
            for template in templates:
                piece = assets.instance(template, f"{name}/{template.name}", collection)
                piece.parent = obj
                piece.matrix_world = template.matrix_world.copy()
            return (obj, note, True)

        imported = assets.import_asset(url)
        if not imported:
            return (None, f"nothing imported from {url}", False)
        roots = [o for o in imported if o.parent is None] or imported
        if len(roots) == 1:
            obj = assets.instance(roots[0], name, collection, children=True)
        else:
            obj = bpy.data.objects.new(name, None)
            obj.empty_display_size = 0.2
            collection.objects.link(obj)
            for root in roots:
                child = assets.instance(root, f"{name}/{root.name}", collection, children=True)
                child.parent = obj
                if world_placed:
                    # Keep each element's own world coordinates (BIM/CAD models
                    # place every element already); parenting alone would leave
                    # them there, but be explicit against a non-identity wrapper.
                    child.matrix_world = root.matrix_world.copy()
        return (obj, "", world_placed)
    except IOError as exc:
        return (None, str(exc), False)
    except Exception as exc:  # pragma: no cover - importer-specific failures
        return (None, f"{url}: {exc}", False)


def _splat_object(
    url: str,
    name: str,
    collection: "bpy.types.Collection",
    settings: Settings,
) -> Tuple[Optional["bpy.types.Object"], str]:
    """A Gaussian splat, wrapped in an empty so placement never touches it.

    The empty is what gets positioned; the splat is parented to it, and its own
    matrix (and therefore its stored Gaussian attributes) is left untouched.
    Returns ``(empty, note)``.
    """
    try:
        objects, note, via_addon = assets.import_splat_asset(url, limit=settings.splat_points)
    except IOError as exc:
        return (None, str(exc))
    if not objects:
        return (None, note or f"no splat imported from {url}")

    empty = bpy.data.objects.new(name, None)
    empty.empty_display_type = "SPHERE"
    empty.empty_display_size = 0.3
    collection.objects.link(empty)
    empty["rete:splatGroup"] = url
    for obj in objects:
        if not via_addon and obj.name not in collection.objects:
            collection.objects.link(obj)  # our preview arrives unlinked
        obj.parent = empty  # deliberately no matrix change — see the docstring
    return (empty, note)


def _geometry_object(
    geom: Optional[geometry.Geometry],
    placement: geometry.Placement,
    name: str,
    settings: Settings,
) -> Optional["bpy.types.Object"]:
    """A mesh built from the geometry literal, or a placeholder marker."""
    if geom is not None and geom.kind in (geometry.LINE, geometry.AREA):
        mesh = _mesh_from_geometry(geom, placement, name, extrude=settings.extrude)
        if mesh is not None:
            return bpy.data.objects.new(name, mesh)
    if geom is not None and geom.kind == geometry.POINT and len(geom.coords) > 1:
        mesh = _mesh_from_geometry(geom, placement, name)
        if mesh is not None:
            return bpy.data.objects.new(name, mesh)
    return _primitive(settings.point_style, settings.point_size, name)


def _local_extent(obj: "bpy.types.Object") -> Tuple[float, float, float]:
    """The object's unscaled bounding-box extent, per axis.

    ``Object.bound_box`` is used rather than ``Object.dimensions``: dimensions
    only reflect a new scale after a depsgraph update, so reading it mid-build
    would size everything from stale numbers.
    """
    corners = getattr(obj, "bound_box", None)
    if not corners:
        return (1.0, 1.0, 1.0)
    points = [tuple(c) for c in corners]
    extent = tuple(
        max(p[i] for p in points) - min(p[i] for p in points) for i in range(3)
    )
    return extent if max(extent) > 1e-9 else (1.0, 1.0, 1.0)  # type: ignore[return-value]


def _fit_to_size(obj: "bpy.types.Object", size: Sequence[float]) -> None:
    """Scale a marker so its bounding box matches a real-world extent."""
    extent = _local_extent(obj)
    obj.scale = tuple(
        max(1e-5, size[i]) / extent[i] if extent[i] > 1e-9 else 1.0 for i in range(3)
    )


def _box_extent(box: geometry.Geometry, placement: geometry.Placement) -> Tuple[float, float, float]:
    lo, hi = box.rings[0][0], box.rings[0][1]
    return placement.apply_size(tuple(hi[i] - lo[i] for i in range(3)))  # type: ignore[arg-type]


def _place(
    obj: "bpy.types.Object",
    position: Optional[Tuple[float, float, float]],
    geom: Optional[geometry.Geometry],
    box: Optional[geometry.Geometry],
    placement: geometry.Placement,
    settings: Settings,
    index: int,
    total: int,
    row: Dict,
    binding: detect.Binding,
    *,
    keep_transform: bool = False,
) -> None:
    """Position one object: geometry, scatter, layout, or leave it alone.

    ``keep_transform`` marks an object whose transform is already meaningful —
    a node isolated from a shared asset sits where the asset puts it, and moving
    it to the row's centroid would apply the placement twice.
    """
    layout = settings.layout

    if layout == "SCATTER":
        obj.location = _scatter_position(row, binding, settings)
        return
    if layout in ("GRID", "CIRCLE"):
        obj.location = _layout_position(index, total, settings)
        return
    if layout == "NONE" or keep_transform:
        return

    if position is not None:
        obj.location = placement.apply(position)
        if obj.type == "MESH" and settings.point_style in ("CUBE", "SPHERE"):
            sizer = geom if (geom is not None and geom.kind == geometry.BOX) else box
            if sizer is not None:
                _fit_to_size(obj, _box_extent(sizer, placement))
        return

    if layout == "AUTO":
        obj.location = _layout_position(index, total, settings)


def _axis_coords(row: Dict, binding: detect.Binding) -> Optional[Tuple[float, float, float]]:
    """A position from columns named after coordinate axes (x/y/z, lon/lat/alt)."""
    axes = binding.xyz
    if not axes:
        return None
    out = [0.0, 0.0, 0.0]
    found = False
    for axis, var in enumerate(axes):
        if not var:
            continue
        cell = row.get(var)
        value = cell.as_number() if cell is not None else None
        if value is not None:
            out[axis] = value
            found = True
    return (out[0], out[1], out[2]) if found else None


def _scatter_position(row: Dict, binding: detect.Binding, settings: Settings) -> Tuple[float, float, float]:
    """XYZ from up to three numeric columns — a 3D scatter plot of the data.

    Deliberately unscaled and unrecentred: a scatter plot is a plot, and its
    axes should stay in the data's own units.
    """
    chosen = [v for v in settings.scatter_vars if v] or [v for v in binding.xyz if v]
    chosen = chosen or binding.numbers[:3]
    out = [0.0, 0.0, 0.0]
    for axis, var in enumerate(chosen[:3]):
        cell = row.get(var)
        if cell is not None:
            value = cell.as_number()
            if value is not None:
                out[axis] = value
    return (out[0], out[1], out[2])


def _path_group_var(rows: Sequence[Dict], binding: detect.Binding) -> Optional[str]:
    """Which column names the thing that moves.

    Among the IRI-bearing columns, the one with the fewest distinct values is
    the moving object; the row's own subject is unique per sample and would
    produce one motionless object per keyframe.
    """
    candidates = binding.all_of(detect.ENTITY, detect.CLASS, detect.LABEL)
    best: Optional[str] = None
    best_distinct = len(rows) + 1
    for var in candidates:
        distinct = {c.value for c in (row.get(var) for row in rows) if c is not None}
        if 0 < len(distinct) < best_distinct:
            best, best_distinct = var, len(distinct)
    # Only worth using if it actually groups the rows.
    return best if best_distinct < len(rows) else None


def _apply_materials(
    rows: Sequence[Dict],
    by_iri: Dict[str, "bpy.types.Object"],
    binding: detect.Binding,
    entity_var: Optional[str],
    settings: Settings,
    report: Report,
) -> int:
    """Colour the scene: explicit colour, texture, numeric ramp, or class."""
    mode = settings.material_mode
    if mode == "NONE":
        return 0

    color_var = binding.color
    image_var = binding.image
    class_var = binding.klass

    # A chosen column colours the scene either as a ramp or as categories,
    # depending on what it holds — "colour by tissue type" is as useful as
    # "colour by mass", and the user should not have to say which kind it is.
    chosen = settings.material_var
    chosen_numeric = bool(chosen) and binding.roles.get(chosen) == detect.NUMBER
    number_var = chosen if chosen_numeric else ""
    category_var = chosen if (chosen and not chosen_numeric) else ""
    if not number_var and not category_var and binding.numbers:
        number_var = binding.numbers[0]

    if mode == "NUMBER" and not number_var:
        report.warn("no numeric column to colour by")
        return 0

    values: List[float] = []
    if mode in ("AUTO", "NUMBER") and number_var:
        for row in rows:
            cell = row.get(number_var)
            number = cell.as_number() if cell is not None else None
            if number is not None:
                values.append(number)
    low, high = (min(values), max(values)) if values else (0.0, 1.0)
    span = high - low

    applied = 0
    for row in rows:
        iri = row[entity_var].value if entity_var and row.get(entity_var) is not None else ""
        obj = by_iri.get(iri)
        if obj is None:
            continue

        mat = None
        if mode in ("AUTO", "COLOR") and color_var and row.get(color_var) is not None:
            rgba = materials.parse_color(row[color_var].value)
            if rgba:
                mat = materials.solid(f"c:{row[color_var].value}", rgba)
        if mat is None and mode in ("AUTO", "TEXTURE") and image_var and row.get(image_var) is not None:
            mat = materials.textured(row[image_var].value, max_pixels=settings.texture_size)
        if mat is None and mode in ("AUTO", "NUMBER") and number_var and row.get(number_var) is not None:
            number = row[number_var].as_number()
            if number is not None:
                t = 0.5 if span < 1e-12 else (number - low) / span
                mat = materials.solid(f"n:{number_var}:{t:.3f}", materials.ramp(t))
        if mat is None and category_var and row.get(category_var) is not None:
            value = row[category_var].value
            mat = materials.solid(f"cat:{value}", materials.color_for_key(value))
        if mat is None and mode in ("AUTO", "CLASS"):
            key = ""
            if class_var and row.get(class_var) is not None:
                key = row[class_var].value
            elif rprops.classes_of(obj):
                key = rprops.classes_of(obj)[0]
            if key:
                mat = materials.solid(f"k:{rprops.local_name(key)}", materials.color_for_key(key))

        if mat is not None:
            materials.assign(obj, mat)
            applied += 1
    return applied


def _apply_time(
    rows: Sequence[Dict],
    by_iri: Dict[str, "bpy.types.Object"],
    entity_var: Optional[str],
    times: Sequence[Optional[float]],
    ends: Sequence[Optional[float]],
    mapper: timeline.Mapper,
    paths: Dict[str, List[Tuple[float, Sequence[float]]]],
    settings: Settings,
) -> int:
    """Put the scene on the timeline in whichever mode was chosen."""
    mode = settings.time_mode
    touched = 0

    if mode == "PATH":
        for key, frames in paths.items():
            obj = by_iri.get(key)
            if obj is not None and len(frames) > 1:
                timeline.key_location(obj, frames)
                touched += 1
        return touched

    seen: set = set()
    for index, row in enumerate(rows):
        iri = row[entity_var].value if entity_var and row.get(entity_var) is not None else ""
        obj = by_iri.get(iri)
        if obj is None or iri in seen:
            continue
        seen.add(iri)
        start = mapper.frame(times[index])
        end = mapper.frame(ends[index]) if ends[index] is not None else None
        if start is None and end is None:
            continue
        if mode == "APPEAR":
            timeline.key_visibility(
                obj, start, end, frame_start=settings.frame_start, frame_end=settings.frame_end
            )
        elif mode == "GROW":
            timeline.key_grow(obj, start, frame_start=settings.frame_start)
        elif mode == "RETIME" and start is not None:
            timeline.retime_action(obj, start - settings.frame_start)
        touched += 1
    return touched


def _apply_physics(by_iri: Dict[str, "bpy.types.Object"], settings: Settings, report: Report) -> None:
    objects = list(by_iri.values())
    body_type = "PASSIVE" if settings.physics_mode == "PASSIVE" else "ACTIVE"
    report.bodies = physics.add_bodies(objects, body_type=body_type, shape=settings.physics_shape)
    if report.bodies == 0:
        report.warn("no mesh objects to give rigid bodies to")
        return
    if settings.physics_mass_var:
        physics.scale_masses(objects, rprops.prop_key(settings.physics_mass_var))
    if settings.constraint_predicate:
        try:
            edges = relations.fetch_edges(settings.source, by_iri, settings.constraint_predicate)
            report.constraints = physics.constraint_network(
                [(a, b) for a, b, _ in edges if b is not None],
                constraint_type=settings.constraint_type,
            )
            if report.constraints == 0:
                report.warn("the constraint predicate matched no pairs in the scene")
        except Exception as exc:
            report.warn(f"constraint pass failed: {exc}")


def _build_point_cloud(
    rows: Sequence[Dict],
    result,
    binding: detect.Binding,
    roles: Dict[str, str],
    placement: geometry.Placement,
    raw_positions: Sequence[Optional[Tuple[float, float, float]]],
    settings: Settings,
    report: Report,
) -> Report:
    """The scale path: one attributed mesh instead of one object per row."""
    positions: List[Tuple[float, float, float]] = []
    for index, raw in enumerate(raw_positions):
        if settings.layout == "SCATTER":
            positions.append(_scatter_position(rows[index], binding, settings))
        elif raw is not None:
            positions.append(placement.apply(raw))
        else:
            positions.append(_layout_position(index, len(rows), settings))

    columns = {var: [row.get(var) for row in rows] for var in result.vars}
    obj = attributes.build_point_cloud(
        f"{settings.collection_name} points",
        positions,
        columns,
        report.collection or get_collection(settings.collection_name, bpy.context.scene),
        roles=roles,
    )
    obj[rprops.SOURCE] = settings.source
    obj[rprops.QUERY] = settings.query
    scale_attr = rprops.prop_key(binding.numbers[0]) if binding.numbers else ""
    attributes.add_instancer(obj, scale_attribute=scale_attr, radius=settings.point_size)
    report.objects = 1
    report.warn(f"{len(rows)} rows written as point attributes for Geometry Nodes")
    return report
