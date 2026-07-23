"""Fetching and importing the 3D assets and images a graph points at.

Datasets store assets as plain URLs, so the add-on has to be a small download
manager: cache on disk (keyed by URL, so a re-import or a second session costs
nothing), import once per distinct URL, and hand out linked copies afterwards.

That last part is what makes the anatomy case work: 4,884 structures reference
nine shared body-system ``.glb`` files, and each structure is one *node* inside
one of them. Importing each system once and linking each structure's node keeps
the whole body at a few thousand objects sharing nine meshes' worth of data,
instead of downloading a 30 MB file per row.
"""

from __future__ import annotations

import hashlib
import os
import urllib.error
import urllib.parse
import urllib.request
from typing import Dict, List, Optional, Tuple

import bpy

from . import detect, splats
from .splats import is_splat_ply  # noqa: F401 — re-exported for callers

USER_AGENT = "rete-blender (+https://github.com/caviri/rete)"
TIMEOUT = 60

#: url -> local file path
_downloads: Dict[str, str] = {}
#: url -> objects imported from it, kept in the hidden library collection
_library: Dict[str, List[str]] = {}
#: (url, node) -> object name
_nodes: Dict[Tuple[str, str], str] = {}

LIBRARY_COLLECTION = "rete assets (source)"


def cache_dir() -> str:
    """Where downloaded assets live between sessions."""
    path = bpy.utils.user_resource("DATAFILES", path=os.path.join("rete", "cache"), create=True)
    return path or bpy.app.tempdir


def clear_cache() -> int:
    """Delete every cached download. Returns the number of files removed."""
    removed = 0
    directory = cache_dir()
    for name in os.listdir(directory):
        try:
            os.remove(os.path.join(directory, name))
            removed += 1
        except OSError:
            pass
    _downloads.clear()
    return removed


def _cache_path(url: str) -> str:
    digest = hashlib.sha256(url.encode("utf-8")).hexdigest()[:20]
    ext = detect.url_extension(url) or ".bin"
    return os.path.join(cache_dir(), f"{digest}{ext}")


def fetch(url: str, *, refresh: bool = False) -> str:
    """Resolve a URL (or local path) to a readable local file path.

    Raises ``IOError`` with the URL in the message on failure — the caller
    reports it per row and carries on rather than aborting the import.
    """
    if not url:
        raise IOError("empty asset URL")
    # Fragments address a moment or a view inside the asset, not a different
    # file — the dance graph appends "#t=8.3" to seek. Dropping it keeps one
    # cache entry per real file instead of one per referenced instant.
    url = url.split("#", 1)[0]
    if not url.startswith(("http://", "https://")):
        path = url[7:] if url.startswith("file://") else url
        path = bpy.path.abspath(path)
        if not os.path.exists(path):
            raise IOError(f"asset not found: {path}")
        return path

    if not refresh and url in _downloads and os.path.exists(_downloads[url]):
        return _downloads[url]

    target = _cache_path(url)
    if not refresh and os.path.exists(target) and os.path.getsize(target) > 0:
        _downloads[url] = target
        return target

    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            data = response.read()
    except urllib.error.HTTPError as exc:
        raise IOError(f"HTTP {exc.code} fetching {url}") from exc
    except Exception as exc:
        raise IOError(f"could not fetch {url}: {exc}") from exc
    if not data:
        raise IOError(f"empty response from {url}")
    with open(target, "wb") as fh:
        fh.write(data)
    _downloads[url] = target
    return target


# ------------------------------------------------------------------ importing


def _importer(path: str):
    """The Blender operator for a file, or ``None`` if unsupported here.

    Importer operator names moved around across 3.x/4.x, so each family lists
    its candidates newest first and the first one present wins.
    """
    family = detect.MODEL_EXT.get(os.path.splitext(path)[1].lower())
    candidates = {
        "gltf": [("import_scene", "gltf")],
        "obj": [("wm", "obj_import"), ("import_scene", "obj")],
        "fbx": [("import_scene", "fbx")],
        "stl": [("wm", "stl_import"), ("import_mesh", "stl")],
        "ply": [("wm", "ply_import"), ("import_mesh", "ply")],
        "usd": [("wm", "usd_import")],
        "alembic": [("wm", "alembic_import")],
        "collada": [("wm", "collada_import")],
        "x3d": [("import_scene", "x3d")],
        "svg": [("import_curve", "svg")],
    }.get(family or "", [])
    for module, name in candidates:
        group = getattr(bpy.ops, module, None)
        # `dir()` on an operator group lists what is actually registered, which
        # is the only reliable presence test — `getattr` succeeds regardless.
        if group is not None and name in dir(group):
            return getattr(group, name)
    return None


def library_collection() -> "bpy.types.Collection":
    """The hidden collection holding one pristine import per asset URL."""
    coll = bpy.data.collections.get(LIBRARY_COLLECTION)
    if coll is None:
        coll = bpy.data.collections.new(LIBRARY_COLLECTION)
        bpy.context.scene.collection.children.link(coll)
        # Excluded from the view layer: it is source data, not scene content.
        layer = bpy.context.view_layer.layer_collection.children.get(LIBRARY_COLLECTION)
        if layer:
            layer.exclude = True
    return coll


def _family(path: str) -> str:
    ext = os.path.splitext(path)[1].lower()
    if ext in detect.SPLAT_EXT:
        return "splat"
    return detect.MODEL_EXT.get(ext, "")


#: url -> whether it is a Gaussian splat (cached; a ``.ply`` costs one sniff).
_splat_sniff: Dict[str, bool] = {}


def is_splat_asset(url: str) -> bool:
    """Whether a URL is a Gaussian splat, sniffing ``.ply`` headers when needed.

    ``.splat``/``.ksplat``/``.spz`` are splat-only by extension; a ``.ply`` may be
    a plain mesh or a splat, told apart by its header — so a splat exported as
    ``.ply`` still gets splat handling.
    """
    url = url.split("#", 1)[0]
    if url in _splat_sniff:
        return _splat_sniff[url]
    ext = detect.url_extension(url)
    result = False
    if ext in detect.SPLAT_EXT:
        result = True
    elif ext == ".ply":
        try:
            result = is_splat_ply(fetch(url))
        except IOError:
            result = False
    _splat_sniff[url] = result
    return result


def import_asset(url: str, *, refresh: bool = False) -> List["bpy.types.Object"]:
    """Import an asset URL once, returning the objects it produced.

    Repeat calls return the same objects; use :func:`instance` to place copies.
    """
    if not refresh and url in _library:
        objs = [bpy.data.objects.get(n) for n in _library[url]]
        objs = [o for o in objs if o is not None]
        if objs:
            return objs

    path = fetch(url, refresh=refresh)
    family = _family(path)

    before = set(bpy.data.objects)
    if family == "splat" or (family == "ply" and is_splat_ply(path)):
        objects, _, via_addon = splats.import_splat(path, url)
        if not via_addon:
            for obj in objects:  # preview objects arrive unlinked
                bpy.context.scene.collection.objects.link(obj)
        _library[url] = [o.name for o in objects]
        return objects
    elif family in detect.CAD_FAMILIES:
        new = _import_cad(path, family, url)
    else:
        op = _importer(path)
        if op is None:
            raise IOError(f"no importer available for {os.path.basename(path)}")
        try:
            op(filepath=path)
        except RuntimeError as exc:
            raise IOError(f"import failed for {url}: {exc}") from exc
        new = [o for o in bpy.data.objects if o not in before]

    coll = library_collection()
    for obj in new:
        for existing in list(obj.users_collection):
            existing.objects.unlink(obj)
        coll.objects.link(obj)

    _library[url] = [o.name for o in new]
    for obj in new:
        _nodes[(url, obj.name)] = obj.name
    return new


def import_splat_asset(url: str, *, refresh: bool = False, limit: int = None) -> Tuple[List["bpy.types.Object"], str, bool]:
    """Import a Gaussian splat, returning ``(objects, note, via_addon)``.

    Separate from :func:`import_asset` because the splat path carries a note (the
    preview / add-on message), reports whether an add-on handled it, and its
    objects are used in place, never instanced — copying a splat object would
    risk desyncing its attributes. Objects are left unlinked (preview) or as the
    add-on placed them (via_addon); the builder does the linking and placement.
    """
    url = url.split("#", 1)[0]
    if not refresh and url in _library:
        objs = [bpy.data.objects.get(n) for n in _library[url] if bpy.data.objects.get(n)]
        if objs:
            return objs, "", False
    path = fetch(url, refresh=refresh)
    kwargs = {"limit": limit} if limit else {}
    objects, note, via_addon = splats.import_splat(path, url, **kwargs)
    _library[url] = [o.name for o in objects]
    return objects, note, via_addon


# --------------------------------------------------------------- CAD / BIM

IFC_HINT = (
    "IFC import needs ifcopenshell in Blender's Python. Install it with "
    "`<blender-python> -m pip install ifcopenshell`, or install the Bonsai "
    "(BlenderBIM) add-on. Many CAD graphs also carry a cad:glbModel column, "
    "which imports with no extra install."
)


def _import_cad(path: str, family: str, url: str) -> List["bpy.types.Object"]:
    if family == "ifc":
        return _import_ifc(path)
    if family == "dxf":
        return _import_dxf(path, url)
    if family == "step":
        raise IOError(
            f"STEP import ({os.path.basename(path)}) has no importer in core "
            "Blender; convert the model to glTF or IFC, or point the graph at a "
            "cad:glbModel instead"
        )
    raise IOError(f"unsupported CAD family {family!r}")


def _import_dxf(path: str, url: str) -> List["bpy.types.Object"]:
    """Import a DXF via the add-on Blender ships (disabled by default)."""
    before = set(bpy.data.objects)
    op = getattr(bpy.ops.import_scene, "dxf", None)
    if op is None or "dxf" not in dir(bpy.ops.import_scene):
        try:
            import addon_utils

            addon_utils.enable("io_import_dxf", default_set=False, persistent=False)
        except Exception as exc:  # pragma: no cover - depends on the build
            raise IOError(
                f"DXF import needs the 'Import AutoCAD DXF' add-on, which could "
                f"not be enabled: {exc}"
            ) from exc
    try:
        bpy.ops.import_scene.dxf(filepath=path)
    except (RuntimeError, AttributeError) as exc:
        raise IOError(f"DXF import failed for {url}: {exc}") from exc
    return [o for o in bpy.data.objects if o not in before]


def _ifc_settings(geom):
    """A world-coordinate geometry settings object across ifcopenshell versions.

    0.8 keys settings by string; earlier releases used enum attributes. World
    coordinates bake each element's placement into its vertices, so the imported
    building lands assembled without a separate transform pass.
    """
    settings = geom.settings()
    for key in ("use-world-coords",):
        try:
            settings.set(key, True)
            return settings
        except (TypeError, RuntimeError):
            pass
    enum = getattr(settings, "USE_WORLD_COORDS", None)
    if enum is not None:
        settings.set(enum, True)
    return settings


def _import_ifc(path: str) -> List["bpy.types.Object"]:
    """Tessellate an IFC model into Blender meshes with ifcopenshell.

    One object per IFC element, named by class and name, carrying its GlobalId
    and IFC class as custom properties so the BIM identity survives even on the
    imported-asset path. Falls back to the Bonsai add-on's project loader when
    ifcopenshell is not importable but Bonsai is installed.
    """
    try:
        import ifcopenshell
        import ifcopenshell.geom as geom
    except Exception:
        objs = _import_ifc_via_bonsai(path)
        if objs is not None:
            return objs
        raise IOError(IFC_HINT)

    try:
        model = ifcopenshell.open(path)
    except Exception as exc:
        raise IOError(f"could not open IFC {os.path.basename(path)}: {exc}") from exc

    settings = _ifc_settings(geom)
    try:
        iterator = geom.iterator(settings, model)
    except Exception as exc:
        raise IOError(f"IFC geometry iterator failed: {exc}") from exc

    created: List["bpy.types.Object"] = []
    if not iterator.initialize():
        raise IOError(f"IFC model has no renderable geometry: {os.path.basename(path)}")
    while True:
        shape = iterator.get()
        obj = _ifc_shape_to_object(model, shape)
        if obj is not None:
            created.append(obj)
        if not iterator.next():
            break
    if not created:
        raise IOError(f"no geometry tessellated from {os.path.basename(path)}")
    return created


def _ifc_shape_to_object(model, shape) -> Optional["bpy.types.Object"]:
    geometry = shape.geometry
    verts = list(geometry.verts)
    faces = list(geometry.faces)
    if not verts or not faces:
        return None
    coords = [(verts[i], verts[i + 1], verts[i + 2]) for i in range(0, len(verts), 3)]
    tris = [(faces[i], faces[i + 1], faces[i + 2]) for i in range(0, len(faces), 3)]

    # The iterator's element carries class/name/guid directly; a model lookup is
    # only the fallback, so this stays correct across ifcopenshell versions.
    ifc_class = getattr(shape, "type", "") or ""
    name = getattr(shape, "name", "") or ""
    guid = getattr(shape, "guid", "") or ""
    if not ifc_class:
        try:
            element = model.by_id(shape.id)
            ifc_class = element.is_a()
            name = name or (getattr(element, "Name", None) or "")
            guid = guid or (getattr(element, "GlobalId", "") or "")
        except Exception:
            ifc_class = "IfcElement"
    label = f"{ifc_class}:{name}" if name else f"{ifc_class}:{(guid[:8] or shape.id)}"

    mesh = bpy.data.meshes.new(label)
    mesh.from_pydata(coords, [], tris)
    mesh.update()
    obj = bpy.data.objects.new(label, mesh)
    # BIM identity as custom properties, so the imported asset is still queryable
    # and colour-by-class works even without the graph's own columns.
    if guid:
        obj["ifcGuid"] = guid
    obj["ifcClass"] = ifc_class
    return obj


def _import_ifc_via_bonsai(path: str) -> Optional[List["bpy.types.Object"]]:
    """Load an IFC through the Bonsai add-on if it is installed, else ``None``."""
    loader = getattr(getattr(bpy.ops, "bim", None), "load_project", None)
    if loader is None or "load_project" not in dir(getattr(bpy.ops, "bim", object())):
        return None
    before = set(bpy.data.objects)
    try:
        bpy.ops.bim.load_project(filepath=path)
    except (RuntimeError, AttributeError):
        return None
    return [o for o in bpy.data.objects if o not in before] or None


def _strip_blender_suffix(name: str) -> str:
    """Drop Blender's ``.001`` uniquifying suffix, if present."""
    if len(name) > 4 and name[-4] == "." and name[-3:].isdigit():
        return name[:-4]
    return name


def _normalise(name: str) -> str:
    return _strip_blender_suffix(name).replace(" ", "_").replace("-", "_").lower()


def _split_suffix(name: str) -> Tuple[str, str]:
    """``"Biceps.l"`` -> ``("Biceps", "l")``; no dot -> ``(name, "")``."""
    base, dot, suffix = name.rpartition(".")
    return (base, suffix) if dot and len(suffix) <= 4 else (name, "")


def find_nodes(url: str, node: str) -> Tuple[List["bpy.types.Object"], bool]:
    """Nodes matching a name inside an asset, and whether the match was exact.

    Matching is tiered, because a graph's node names and an exporter's object
    names agree less often than you would hope:

    1. the name exactly as given;
    2. the same name normalised for case, spaces and Blender's ``.001`` suffix;
    3. the same base name with a *related* suffix on the same body side.

    Tier 3 is what rescues the anatomy graph, whose per-system files hold a
    structure's constituent pieces — a muscle's origin and insertion decals,
    ``Name.ol`` and ``Name.el`` — rather than a single node called ``Name.l``.
    Every piece is returned, so the caller can assemble the whole structure.
    """
    objs = import_asset(url)
    if not objs:
        return ([], False)

    cached = _nodes.get((url, node))
    if cached:
        obj = bpy.data.objects.get(cached)
        if obj is not None:
            return ([obj], True)

    exact = [o for o in objs if o.name == node]
    if exact:
        _nodes[(url, node)] = exact[0].name
        return (exact, True)

    wanted = _normalise(node)
    near = [o for o in objs if _normalise(o.name) == wanted]
    if near:
        _nodes[(url, node)] = near[0].name
        return (near, True)

    base, side = _split_suffix(node)
    if not base:
        return ([], False)
    base_norm = _normalise(base)
    related = []
    for obj in objs:
        obj_base, obj_side = _split_suffix(_strip_blender_suffix(obj.name))
        if _normalise(obj_base) != base_norm:
            continue
        # Keep the body side: a left structure must not pick up a right one.
        if side and obj_side and not obj_side.endswith(side):
            continue
        related.append(obj)
    return (related, False)


def find_node(url: str, node: str) -> Optional["bpy.types.Object"]:
    """The single best node matching a name, or ``None``."""
    found, _ = find_nodes(url, node)
    return found[0] if found else None


def instance(
    template: "bpy.types.Object",
    name: str,
    collection: "bpy.types.Collection",
    *,
    children: bool = False,
) -> "bpy.types.Object":
    """A linked copy of ``template`` — new object, shared mesh data.

    Sharing the data block is what keeps thousands of rows affordable: the
    copies cost an object each, not a mesh each.
    """
    copy = template.copy()
    copy.name = name
    copy.animation_data_clear()
    collection.objects.link(copy)
    if children:
        for child in template.children:
            child_copy = instance(child, f"{name}/{child.name}", collection, children=True)
            child_copy.parent = copy
            child_copy.matrix_parent_inverse = child.matrix_parent_inverse.copy()
    return copy


# ------------------------------------------------------------------- images


def load_image(url: str, *, max_pixels: int = 2048) -> Optional["bpy.types.Image"]:
    """Download an image and load it as a Blender image datablock.

    IIIF URLs are rewritten to ask the server for a bounded size instead of the
    full-resolution scan — a texture does not need 8000 pixels, and the
    difference is tens of megabytes per row.
    """
    url = iiif_resize(url, max_pixels)
    existing = bpy.data.images.get(_image_key(url))
    if existing is not None:
        return existing
    try:
        path = fetch(url)
    except IOError:
        return None
    try:
        image = bpy.data.images.load(path, check_existing=True)
    except RuntimeError:
        return None
    image.name = _image_key(url)
    return image


def _image_key(url: str) -> str:
    return "rete:" + hashlib.sha256(url.encode("utf-8")).hexdigest()[:12]


_IIIF_SIZE_SEGMENTS = ("full", "max")


def iiif_resize(url: str, max_pixels: int) -> str:
    """Ask a IIIF Image API endpoint for a bounded size.

    ``.../full/full/0/default.jpg`` becomes ``.../full/!N,N/0/default.jpg``.
    Non-IIIF URLs pass through untouched.
    """
    if not detect._IIIF_RE.search(url or ""):
        return url
    parts = url.split("/")
    # The size segment is third from the end: region/SIZE/rotation/quality.ext
    if len(parts) >= 4 and parts[-3] in _IIIF_SIZE_SEGMENTS:
        parts[-3] = f"!{max_pixels},{max_pixels}"
        return "/".join(parts)
    return url
